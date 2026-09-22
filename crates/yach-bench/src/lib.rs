pub mod eval_review;
pub mod fixtures;
pub mod latency;
pub mod perf;
pub mod replay;

#[cfg(test)]
#[global_allocator]
static TEST_ALLOC: crate::perf::alloc::Counting = crate::perf::alloc::Counting;
