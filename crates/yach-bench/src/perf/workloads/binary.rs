use crate::perf::registry::{Bin, Measured, Requirement, Workload};
use crate::perf::schema::{Class, Isolation};

pub static BINARY: [Workload; 1] = [Workload {
    id: "binary/size_bytes",
    class: Class::Size,
    isolation: Isolation::ChildProcess,
    requires: &[Requirement::Binary],
    bin: Some(Bin::Shipping),
    run: |ctx| {
        let path = ctx.yach_bin.as_ref().ok_or("yach binary path missing")?;
        let len = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
        Ok(Measured::Value(len))
    },
}];
