//! Configuration types for durable operations.

type ItemNamer<TIn> = dyn Fn(&TIn, usize) -> String + Send + Sync;
type WaitStrategy<T> = dyn Fn(&T, u32) -> WaitConditionDecision + Send + Sync;

mod callback;
mod child;
mod completion;
mod durable_execution;
mod invoke;
mod map;
mod parallel;
mod step;
mod wait_condition;

pub use callback::*;
pub use child::*;
pub use completion::*;
pub use durable_execution::*;
pub use invoke::*;
pub use map::*;
pub use parallel::*;
pub use step::*;
pub use wait_condition::*;

#[cfg(test)]
mod tests;
