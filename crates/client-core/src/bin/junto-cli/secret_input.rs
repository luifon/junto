//! Reading a code: from standard input when it is not a terminal, and from the terminal
//! with echo off otherwise. A code is never taken from the command line, where other
//! programs and the shell history would see it.
//!
//! A line is read byte by byte into one buffer of a fixed size, which is overwritten when
//! it is dropped. Nothing is read beyond the end of the line, no buffer grows, and a line
//! that is longer than its limit is an error.

use std::io::{self, IsTerminal, Read, Write};

use zeroize::{Zeroize, Zeroizing};

/// The longest owner, invite or link code that the tool reads, in bytes.
pub const LONGEST_CODE: usize = 256;

/// Reads one line of at most `limit` bytes from `input` and returns it without its line
/// end and outer blanks. The end of the input also ends the line.
pub fn read_line_from(input: &mut impl Read, limit: usize) -> io::Result<Zeroizing<String>> {
    let mut buffer = Zeroizing::new(vec![0u8; limit]);
    let mut length = 0;
    let mut byte = [0u8; 1];
    let outcome = loop {
        match input.read(&mut byte) {
            Ok(0) => break Ok(()),
            Ok(_) if byte[0] == b'\n' => break Ok(()),
            Ok(_) if length == limit => {
                break Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("the line is longer than {limit} bytes"),
                ));
            }
            Ok(_) => {
                buffer[length] = byte[0];
                length += 1;
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => break Err(error),
        }
    };
    byte.zeroize();
    outcome?;
    let line = std::str::from_utf8(&buffer[..length])
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "the line is not text"))?
        .trim();
    let mut text = Zeroizing::new(String::with_capacity(line.len()));
    text.push_str(line);
    Ok(text)
}

/// One line of standard input.
pub fn read_line(limit: usize) -> io::Result<Zeroizing<String>> {
    read_line_from(&mut raw::Stdin, limit)
}

/// Reads a secret line. On a terminal the echo is turned off first, then the prompt is
/// shown, and the echo is restored after the line was read.
pub fn read_secret(prompt: &str, limit: usize) -> io::Result<Zeroizing<String>> {
    if !io::stdin().is_terminal() {
        return read_line(limit);
    }
    let hidden = echo::Off::set()?;
    eprint!("{prompt}: ");
    io::stderr().flush()?;
    let line = read_line(limit);
    drop(hidden);
    eprintln!();
    line
}

#[cfg(unix)]
mod raw {
    use std::io;

    /// Standard input without the buffer of the standard library, which would hold what
    /// was read where this program cannot overwrite it.
    pub struct Stdin;

    impl io::Read for Stdin {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            Ok(rustix::io::read(rustix::stdio::stdin(), buffer)?)
        }
    }
}

#[cfg(not(unix))]
mod raw {
    use std::io;

    /// Standard input. On this system it goes through the buffer of the standard library.
    pub struct Stdin;

    impl io::Read for Stdin {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            io::Read::read(&mut io::stdin().lock(), buffer)
        }
    }
}

#[cfg(unix)]
mod echo {
    use std::io;

    use rustix::termios::{LocalModes, OptionalActions, Termios, tcgetattr, tcsetattr};

    /// Echo is off while this value lives.
    pub struct Off {
        before: Termios,
    }

    impl Off {
        pub fn set() -> io::Result<Self> {
            let stdin = io::stdin();
            let before = tcgetattr(&stdin)?;
            let mut hidden = before.clone();
            hidden.local_modes.remove(LocalModes::ECHO);
            tcsetattr(&stdin, OptionalActions::Now, &hidden)?;
            Ok(Self { before })
        }
    }

    impl Drop for Off {
        fn drop(&mut self) {
            let _ = tcsetattr(io::stdin(), OptionalActions::Now, &self.before);
        }
    }
}

#[cfg(windows)]
mod echo {
    //! Not executed by any test yet: it is compiled and type-checked only.
    #![allow(unsafe_code)]

    use std::io;

    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::System::Console::{
        ENABLE_ECHO_INPUT, GetConsoleMode, GetStdHandle, STD_INPUT_HANDLE, SetConsoleMode,
    };

    /// Echo is off while this value lives.
    pub struct Off {
        console: HANDLE,
        before: u32,
    }

    impl Off {
        pub fn set() -> io::Result<Self> {
            // SAFETY: the call takes a constant and returns a handle that the process owns.
            let console = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
            let mut before = 0u32;
            // SAFETY: `before` is a valid place for the mode.
            if unsafe { GetConsoleMode(console, &mut before) } == 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: the handle is the console that the mode was read from.
            if unsafe { SetConsoleMode(console, before & !ENABLE_ECHO_INPUT) } == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Self { console, before })
        }
    }

    impl Drop for Off {
        fn drop(&mut self) {
            // SAFETY: the handle and the mode are the ones that `set` read.
            unsafe { SetConsoleMode(self.console, self.before) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Counts the bytes that were taken from the input.
    struct Counted<R> {
        input: R,
        taken: usize,
    }

    impl<R: Read> Read for Counted<R> {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            let count = self.input.read(buffer)?;
            self.taken += count;
            Ok(count)
        }
    }

    #[test]
    fn a_line_is_read_to_its_end_and_nothing_after_it_is_taken() {
        let mut input = Counted {
            input: &b"ABCD-EFGH\nyes\n"[..],
            taken: 0,
        };
        assert_eq!(
            read_line_from(&mut input, 16).unwrap().as_str(),
            "ABCD-EFGH"
        );
        assert_eq!(input.taken, 10);
        assert_eq!(read_line_from(&mut input, 16).unwrap().as_str(), "yes");
        assert_eq!(read_line_from(&mut input, 16).unwrap().as_str(), "");
    }

    #[test]
    fn a_line_without_a_line_end_and_a_line_with_blanks_are_read() {
        assert_eq!(
            read_line_from(&mut &b"ABCD-EFGH"[..], 16).unwrap().as_str(),
            "ABCD-EFGH"
        );
        assert_eq!(
            read_line_from(&mut &b"  ABCD-EFGH \r\nrest"[..], 16)
                .unwrap()
                .as_str(),
            "ABCD-EFGH"
        );
        let exact = [b'A'; 16];
        assert_eq!(read_line_from(&mut &exact[..], 16).unwrap().len(), 16);
    }

    #[test]
    fn a_line_over_the_limit_is_refused_after_one_byte_too_many() {
        let mut endless = Counted {
            input: io::repeat(b'A'),
            taken: 0,
        };
        let error = read_line_from(&mut endless, 256).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("longer than 256 bytes"));
        assert_eq!(endless.taken, 257);
        assert!(!error.to_string().contains("AAAA"));
    }

    #[test]
    fn bytes_that_are_not_text_are_refused() {
        let error = read_line_from(&mut &[0xff, 0xfe, b'\n'][..], 16).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }
}
