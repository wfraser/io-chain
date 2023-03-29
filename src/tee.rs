use std::io::{self, Read, Write};
use std::os::fd::OwnedFd;
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use parking_lot::{Condvar, Mutex, RwLock};

use crate::misc::{read_stream, write_stream, ThreadPanicked};
use crate::{Filter, ReadStream, RunningFilter, WriteStream};

/// [`Tee`] takes input from a [`ReadStream`] and copies it simultaneously to
/// any number of [`Write`] streams.
pub struct Tee {
    threads: Vec<JoinHandle<io::Result<()>>>,
    channels: Vec<SyncSender<Arc<RwLock<Vec<u8>>>>>,
    notify: Arc<(Mutex<usize>, Condvar)>,
    buffer: Arc<RwLock<Vec<u8>>>,
}

impl Tee {
    /// Create a new [`Tee`] with the given buffer size in bytes.
    pub fn new(buffer_size: usize) -> Self {
        Self {
            threads: vec![],
            channels: vec![],
            notify: Arc::new((Mutex::new(0), Condvar::new())),
            buffer: Arc::new(RwLock::new(vec![0; buffer_size])),
        }
    }

    /// Add a destination [`Write`] stream to the tee.
    pub fn add_output(&mut self, mut w: impl Write + Send + 'static) {
        let (tx, rx) = sync_channel(0);
        self.channels.push(tx);
        let mxcv = Arc::clone(&self.notify);
        let notify = move || {
            let (mx, cv) = &*mxcv;
            let mut n = mx.lock();
            *n += 1;
            cv.notify_all();
        };
        let t = thread::spawn(move || {
            while let Ok(buf) = rx.recv() {
                let res = w.write_all(&buf.read());
                notify();
                res?;
            }
            Ok(())
        });
        self.threads.push(t);
    }
}

fn read_loop(mut f: impl Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut cursor = 0;
    loop {
        let n = f.read(&mut buf[cursor..])?;
        cursor += n;
        if n == 0 || cursor == buf.len() {
            return Ok(cursor);
        }
    }
}

impl Filter for Tee {
    type Running = RunningTee;
    type Error = io::Error;

    /// Starts copying `input` to the streams added previously with
    /// [`Tee::add_output()`] and to `output`, all in parallel.
    ///
    /// If all streams were specified already, setting `output` to
    /// `WriteStream::Null` will add no additional overhead.
    fn start(
        mut self,
        input: ReadStream,
        output: WriteStream,
    ) -> Result<Self::Running, Self::Error> {
        let (mut in_rx, in_tx) = read_stream(input)?;
        let mut output_pipe = None;

        if !matches!(output, WriteStream::Null) {
            let (out_tx, out_rx) = write_stream(output)?;
            self.add_output(out_tx);
            output_pipe = out_rx.map(Into::into);
        }

        let buffer = self.buffer;
        let mut channels = self.channels;
        let mut threads = self.threads;
        let t = thread::spawn(move || {
            let (mx, cv) = &*self.notify;
            loop {
                let mut buf_write = buffer.write();
                let n = read_loop(&mut in_rx, &mut buf_write)?;
                if n == 0 {
                    break;
                }
                buf_write.truncate(n);
                drop(buf_write);
                let mut dead = vec![];
                for (i, tx) in channels.iter().enumerate() {
                    if tx.send(Arc::clone(&buffer)).is_err() {
                        dead.push(i);
                    }
                }
                for i in dead.iter().rev() {
                    channels.remove(*i);
                }
                let mut n = mx.lock();
                while *n < channels.len() {
                    cv.wait(&mut n);
                }
                *n = 0;
            }
            Ok(())
        });
        threads.insert(0, t); // wait on this thread before others

        Ok(RunningTee {
            threads,
            input_pipe: in_tx.map(Into::into),
            output_pipe,
        })
    }
}

/// A running instance of a [`Tee`].
pub struct RunningTee {
    threads: Vec<JoinHandle<io::Result<()>>>,
    input_pipe: Option<OwnedFd>,
    output_pipe: Option<OwnedFd>,
}

impl RunningFilter for RunningTee {
    type Result = Vec<io::Result<()>>;

    fn wait(self) -> Self::Result {
        self.threads
            .into_iter()
            .map(|t| t.join().unwrap_or(Err(ThreadPanicked::ioerr())))
            .collect()
    }

    fn input_pipe(&mut self) -> Option<OwnedFd> {
        self.input_pipe.take()
    }

    fn output_pipe(&mut self) -> Option<OwnedFd> {
        self.output_pipe.take()
    }
}
