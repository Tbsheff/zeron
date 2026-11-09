use std::{collections::BTreeMap, path::Path};
use zeron_engine::orchestrator::trace::{self, TraceLine};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut devices: BTreeMap<String, Vec<TraceLine>> = BTreeMap::new();
    for path in std::env::args().skip(1) {
        for row in trace::parse_lines(&std::fs::read_to_string(Path::new(&path))?)? {
            devices.entry(row.dev.clone()).or_default().push(row);
        }
    }
    if devices.is_empty() {
        return Err("no traces supplied".into());
    }
    for (dev, mut rows) in devices {
        rows.sort_by_key(|row| row.seq);
        let report = trace::check(&rows)?;
        println!(
            "Validated {dev}: {} steps (Rust reducer replay)",
            report.steps
        );
    }
    Ok(())
}
