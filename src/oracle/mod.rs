// Oracle (ISO/FS) data layer — in-memory cache + aggregation for the EMH endpoints.
pub mod agg;
pub mod cache;
pub mod model;

pub use cache::OracleCache;
