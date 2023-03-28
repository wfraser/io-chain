use std::error::Error;
use std::fmt::Display;
use std::io::{self, Read, Write};

/// Returned if a copy thread panics, meaning the input or output stream's [`Read::read`] or
/// [`Write::write`] implementation panicked.
#[derive(Debug)]
pub struct ThreadPanicked;

impl Display for ThreadPanicked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("copy thread panicked")
    }
}

impl Error for ThreadPanicked {}

pub(crate) struct DevNull;

impl Write for DevNull {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Read for DevNull {
    fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
        Ok(0)
    }
}
