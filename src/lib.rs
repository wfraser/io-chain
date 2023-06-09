//! This crate provides a set of traits and some simple implementations of chainable I/O filters.
//!
//! Two main types are provided, under a common trait: [`LambdaFilter`] runs in-process (in a
//! background thread), and [`ChildProcess`] runs a command as a child process.
//!
//! Copying data can largely be avoided by using pipes between processes.

#![deny(missing_docs)]

mod lambda;
mod misc;
mod process;
mod tee;
mod traits;

pub use lambda::{Lambda, LambdaFilter, RunningLambda};
pub use process::{ChildProcess, RunningChild};
pub use tee::{RunningTee, Tee};
pub use traits::{Filter, ReadStream, RunningFilter, WriteStream};
