#![cfg(windows)]

extern crate winapi;
extern crate kernel32;

use self::kernel32::*;
use self::winapi::*;
use std::ptr;
use std::iter::repeat;
use std::cmp::min;
use screen::Key;
use screen::Key::*;

use std::thread;
use std::sync::mpsc::Receiver;
use std::sync::mpsc;
use ::NEWLINE;

macro_rules! win32 {
    ($funcall:expr) => (
        if unsafe { $funcall } == 0 {
            panic!("Win32 call failed");
        }
    );
}

pub struct Screen {
    pub height: u16,
    pub width: u16,
    pub visible_choices: u16,
    start_line: u16,
    original_console_mode: DWORD,
    original_colors: WORD,
    input: Receiver<u16>,
    conout: HANDLE,
}

impl Screen {
    pub fn open_screen(desired_rows: u16) -> Screen {
        let mut orig_mode;
        let conin: HANDLE;
        let conout: HANDLE;
        unsafe {
            const OPEN_EXISTING: DWORD = 3;
            let rw_access = GENERIC_READ | GENERIC_WRITE;
            conin = CreateFileA("CONIN$\0".as_ptr() as *const i8, rw_access, FILE_SHARE_READ, ptr::null_mut(), OPEN_EXISTING, 0, ptr::null_mut());
            conout = CreateFileA("CONOUT$\0".as_ptr() as *const i8, rw_access, FILE_SHARE_READ, ptr::null_mut(), OPEN_EXISTING, 0, ptr::null_mut());
            orig_mode = ::std::mem::uninitialized();
        }

        if conin == INVALID_HANDLE_VALUE || conout == INVALID_HANDLE_VALUE {
            panic!("Unable to open console");
        }

        let (cols, rows) = Screen::winsize(conout).unwrap();

        win32!(GetConsoleMode(conin, &mut orig_mode));
        let new_mode = orig_mode & !(ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT);
        win32!(SetConsoleMode(conin, new_mode));

        let rx = Screen::spawn_input_thread(conin as usize);
        let initial_pos = Screen::get_cursor_pos(conout);
        let visible_choices = min(desired_rows, rows - 1);
        let start_line = get_start_line(rows, visible_choices, initial_pos);
        let original_colors = Screen::get_original_colors(conout);
        let (column, _) = initial_pos;
        if column > 0 {
            Self::write_to(conout, NEWLINE);
        }
        for _ in 0..visible_choices {
            Self::write_to(conout, NEWLINE);
        }
        Screen {
            height: rows,
            width: cols,
            visible_choices: visible_choices,
            start_line: start_line + Self::get_buffer_offset(conout),
            original_console_mode: orig_mode,
            original_colors: original_colors,
            input: rx,
            conout: conout,
        }
    }

    // We have to take the conin handle as a usize instead of a *mut c_void in order to avoid a
    // lecture from the compiler about how the latter type cannot be safely sent between threads.
    // I'm not sure if a better solution exists at this time.
    fn spawn_input_thread(conin: usize) -> Receiver<u16> {
        let (tx, rx) = mpsc::channel();

        thread::spawn(move || {
            loop {
                let conin = conin as *mut c_void;
                let mut buf: Vec<u16> = repeat(0u16).take(0x1000).collect();
                let mut chars_read: DWORD = 0;
                win32!(ReadFile(conin, buf.as_mut_ptr() as LPVOID, 1, &mut chars_read as LPDWORD, ptr::null_mut()));
                for i in 0..chars_read as usize {
                    tx.send(buf[i]).unwrap();
                }
            }
        });

        rx
    }

    fn move_cursor(&mut self, line: u16, column: u16) {
        win32!(SetConsoleCursorPosition(self.conout, COORD { X: column as i16, Y: line as i16}));
    }

    pub fn move_cursor_to_prompt_line(&mut self, col: u16) {
        let start_line = self.start_line;
        self.move_cursor(start_line, col);
    }

    pub fn blank_screen(&mut self) {
        let blank_line = repeat(' ').take((self.width - 1) as usize).collect::<String>();
        let start_line = self.start_line;
        self.move_cursor(start_line, 0);
        for _ in 0..self.visible_choices {
            self.write(&blank_line);
            self.write(NEWLINE);
        }
        self.write(&blank_line);
        self.move_cursor(start_line, 0);
    }

    pub fn show_cursor(&mut self) {
        let cursor_info = CONSOLE_CURSOR_INFO { dwSize: 100, bVisible: TRUE };
        win32!(SetConsoleCursorInfo(self.conout, &cursor_info));
    }

    pub fn hide_cursor(&mut self) {
        let cursor_info = CONSOLE_CURSOR_INFO { dwSize: 100, bVisible: FALSE };
        win32!(SetConsoleCursorInfo(self.conout, &cursor_info));
    }

    pub fn write(&mut self, s: &str) {
        Self::write_to(self.conout, s);
    }

    fn write_to(conout: HANDLE, s: &str) {
        let mut chars_written: DWORD = 0;
        let chars_to_write = s.chars().count() as DWORD;
        win32!(WriteConsoleW(conout, Screen::to_wide_char(s), chars_to_write, &mut chars_written as LPDWORD, ptr::null_mut()));
    }

    fn to_wide_char(s: &str) -> PVOID {
        let mut ret = Vec::with_capacity(5001);
        unsafe {
            let buf = ret.as_mut_ptr();
            let _ = MultiByteToWideChar(CP_UTF8, 0, s.as_ptr() as *const i8, s.len() as i32, buf, 2500);
        }
        ret.as_ptr() as PVOID
    }

    pub fn write_red_inverted(&mut self, s: &str) {
        let orig = self.original_colors;
        const WHITE_ON_RED: WORD = BACKGROUND_RED as WORD;
        self.set_colors(WHITE_ON_RED);
        self.write(s);
        self.set_colors(orig);
    }

    pub fn write_red(&mut self, s: &str) {
        let orig = self.original_colors;
        const RED_ON_BLACK: WORD = FOREGROUND_RED as WORD;
        self.set_colors(RED_ON_BLACK);
        self.write(s);
        self.set_colors(orig);
    }

    pub fn write_inverted(&mut self, s: &str) {
        let orig = self.original_colors;
        const BLACK_ON_WHITE: WORD = (BACKGROUND_RED | BACKGROUND_GREEN | BACKGROUND_BLUE) as WORD;
        self.set_colors(BLACK_ON_WHITE);
        self.write(s);
        self.set_colors(orig);
    }

    fn set_colors(&mut self, colors: WORD) {
        win32!(SetConsoleTextAttribute(self.conout, colors));
    }

    pub fn get_buffered_keys(&mut self) -> Vec<Key> {
        let mut ret = Vec::new();
        while let Ok(byte) = self.input.try_recv() {
            ret.push(Screen::translate_byte(byte));
        }
        if ret.is_empty() {
            let byte = self.input.recv().unwrap();
            ret.push(Screen::translate_byte(byte));
        }
        ret
    }

    fn translate_byte(byte: u16) -> Key {
        if byte == '\r' as u16 {
            Enter
        } else if byte == 9 {
            Tab
        } else if byte == 127 {
            Backspace
        } else if byte & 96 == 0 {
            Control(((byte + 96u16) as u8) as char)
        } else {
            Char((byte as u8) as char)
        }
    }

    fn get_cursor_pos(handle: HANDLE) -> (u16, u16) {
        let mut buffer_info = unsafe { ::std::mem::uninitialized() };
        win32!(GetConsoleScreenBufferInfo(handle, &mut buffer_info));
        let cursor_pos = buffer_info.dwCursorPosition;
        (cursor_pos.X as u16, cursor_pos.Y as u16)
    }

    fn get_original_colors(handle: HANDLE) -> WORD {
        let mut buffer_info = unsafe { ::std::mem::uninitialized() };
        win32!(GetConsoleScreenBufferInfo(handle, &mut buffer_info));
        buffer_info.wAttributes
    }

    fn winsize(conout: HANDLE) -> Option<(u16, u16)> {
        let mut buffer_info = unsafe { ::std::mem::uninitialized() };
        let result = unsafe { GetConsoleScreenBufferInfo(conout, &mut buffer_info) };
        if result != 0 {
            // This code specifically computes the size of the window,
            // *not* the size of the buffer (which is easily available
            // from dwSize). I got the algorithm from:
            //
            // http://stackoverflow.com/a/12642749
            let left = buffer_info.srWindow.Left;
            let top = buffer_info.srWindow.Top;
            let right = buffer_info.srWindow.Right;
            let bottom = buffer_info.srWindow.Bottom;
            let cols = right - left + 1;
            let rows = bottom - top + 1;
            Some((cols as u16, rows as u16))
        } else {
            None
        }
    }

    fn get_buffer_offset(conout: HANDLE) -> u16 {
        let mut buffer_info = unsafe { ::std::mem::uninitialized() };
        win32!(GetConsoleScreenBufferInfo(conout, &mut buffer_info));
        buffer_info.srWindow.Top as u16
    }
}

fn get_start_line(rows: u16, visible_choices: u16, initial_pos: (u16, u16)) -> u16 {
    let bottom_most_line = rows - visible_choices - 1;
    let (initial_x, initial_y) = initial_pos;
    let line_under_cursor = if initial_x == 0 { initial_y } else { initial_y + 1 };
    if line_under_cursor + 1 + visible_choices > rows {
        bottom_most_line
    } else {
        line_under_cursor
    }
}

#[cfg(test)]
mod tests {
    use super::kernel32;
    use super::winapi::STD_OUTPUT_HANDLE;
    use super::{Screen, get_start_line};

    #[test]
    fn winsize_test() {
        // AppVeyor builds run without a console, making this test impossible.
        if option_env!("APPVEYOR").is_some() {
            // TODO: It should be made obvious from the output that this test was skipped
            return;
        }
        let conout = unsafe { kernel32::GetStdHandle(STD_OUTPUT_HANDLE) };
        let (cols, rows) = Screen::winsize(conout).expect("Failed to get window size!");
        // We don't know the window size a priori, but we can at least
        // assert that it is within some kind of sensible range.
        assert!(cols > 40);
        assert!(rows > 40);
        assert!(cols < 1000);
        assert!(rows < 1000);
    }

    #[test]
    fn start_line_test() {
        assert_eq!(5, get_start_line(100, 20, (0, 5)));
        assert_eq!(6, get_start_line(100, 20, (1, 5)));
        assert_eq!(79, get_start_line(100, 20, (0, 100)));
        assert_eq!(0, get_start_line(15, 14, ((0, 5))));
        assert_eq!(79, get_start_line(100, 20, (50, 100)));
    }
}
