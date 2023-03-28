//! This crate provides a set of traits and some simple implementations of chainable I/O filters.
//!
//! Two main types are provided, under a common trait: [`Lambda`] runs in-process (in a background
//! thread), and [`ChildProcess`] runs a command as a child process.
//!
//! Copying data can largely be avoided by using pipes between processes.

#![deny(missing_docs)]

mod lambda;
mod misc;
mod process;
mod traits;

pub use lambda::{Lambda, RunningLambda};
pub use process::{ChildProcess, RunningChild};
pub use traits::{Filter, ReadStream, RunningFilter, WriteStream};
