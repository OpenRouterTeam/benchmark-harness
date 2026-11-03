//! Shared building blocks for tool-bench mock APIs: a logical clock, deterministic IDs,
//! an in-memory document store, scenario datasets, request journaling, and HTTP helpers.

pub mod admin;
pub mod clock;
pub mod dataset;
pub mod error;
pub mod grading;
pub mod http;
pub mod ids;
pub mod journal;
pub mod mcp;
pub mod serve;
pub mod service;
pub mod store;
pub mod world;
