use std::io::Write;
use std::os::unix::process::ExitStatusExt;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use io_filters::{ChildProcess, Filter, ReadStream, WriteStream, RunningFilter, Lambda};
use parking_lot::{Mutex, MutexGuard};

#[derive(Debug)]
struct SharedBuf {
    inner: Mutex<Vec<u8>>,
}

impl SharedBuf {
    pub fn new(vec: Vec<u8>) -> Self {
        SharedBuf { inner: Mutex::new(vec) }
    }

    pub fn writer(&self) -> SharedBufWriter<'_> {
        SharedBufWriter {
            inner: self.inner.lock(),
        }
    }

    pub fn bytes(&self) -> Vec<u8> {
        self.inner.lock().clone()
    }
}

struct SharedBufWriter<'a> {
    inner: MutexGuard<'a, Vec<u8>>,
}

impl<'a> Write for SharedBufWriter<'a> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.inner.write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(target_os = "linux")]
#[test]
fn linux_yes_head_sha() {
    let num_bytes = 1024 * 1024 * 512;
    let num_bytes_read = Arc::new(AtomicU64::new(0));
    let num_bytes_write = Arc::clone(&num_bytes_read);

    let yes = ChildProcess::new(Command::new("yes"));
    let head = {
        let mut cmd = Command::new("head");
        cmd.arg("-c").arg(format!("{}", num_bytes));
        ChildProcess::new(cmd)
    };
    let count = Lambda::new(move |buf| {
        num_bytes_write.fetch_add(buf.len() as u64, std::sync::atomic::Ordering::Relaxed);
    });
    let sha = ChildProcess::new(Command::new("sha256sum"));

    let output = Box::leak(Box::new(SharedBuf::new(vec![])));
    let output_writer = Box::new(output.writer());

    let mut yes = yes.start(ReadStream::Null, WriteStream::PipeRequested).unwrap();
    let mut head = head.start(ReadStream::Fd(yes.output_pipe().unwrap()), WriteStream::PipeRequested).unwrap();
    let mut count = count.start(ReadStream::Fd(head.output_pipe().unwrap()), WriteStream::PipeRequested).unwrap();
    let sha = sha.start(ReadStream::Fd(count.output_pipe().unwrap()), WriteStream::Rust(output_writer)).unwrap();

    let (yes, [yes_t1, yes_t2]) = yes.wait();
    assert!(!yes.as_ref().unwrap().success());
    assert_eq!(yes.unwrap().signal(), Some(13));
    assert!(yes_t1.is_none());
    assert!(yes_t2.is_none());

    let (head, [head_t1, head_t2]) = head.wait();
    assert!(head.unwrap().success());
    assert!(head_t1.is_none());
    assert!(head_t2.is_none());

    let count = count.wait();
    assert_eq!(count.unwrap(), num_bytes);
    assert_eq!(num_bytes_read.load(Ordering::SeqCst), num_bytes);

    let (sha, [sha_t1, sha_t2]) = sha.wait();
    assert!(sha.unwrap().success());
    assert!(sha_t1.is_none());
    assert!(sha_t2.unwrap().is_ok());

    let out_str = String::from_utf8_lossy(&output.bytes()).into_owned();
    assert_eq!(out_str, "d227b8c4d59acf0f9711af6049bd5fcde81229cd70093e36ac4f038a14ecf290  -\n");
}