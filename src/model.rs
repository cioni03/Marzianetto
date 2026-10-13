use std::{path::PathBuf, time::SystemTime};

/// La radice di importazione consente di ricostruire le sottocartelle in output.
#[derive(Clone, Debug)]
pub struct Source {
    pub path: PathBuf,
    pub root: PathBuf,
    pub bytes: u64,
    pub modified: Option<SystemTime>,
    pub format: String,
    pub dimensions: [u32; 2],
}

#[derive(Clone, Debug)]
pub struct CompressionResult {
    pub path: PathBuf,
    pub input_bytes: u64,
    pub output_bytes: u64,
    pub warning: Option<String>,
    pub original_retained: bool,
}

impl CompressionResult {
    pub fn saved_bytes(&self) -> i128 {
        self.input_bytes as i128 - self.output_bytes as i128
    }

    pub fn saved_percent(&self) -> f64 {
        savings_percent(self.input_bytes, self.output_bytes)
    }
}

pub fn savings_percent(input: u64, output: u64) -> f64 {
    if input == 0 {
        0.0
    } else {
        (1.0 - output as f64 / input as f64) * 100.0
    }
}

pub fn format_bytes(bytes: u64) -> String {
    let units = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < units.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{size:.2} {}", units[unit])
    }
}

pub fn format_savings(bytes: i128) -> String {
    if bytes < 0 {
        format!("−{}", format_bytes((-bytes) as u64))
    } else {
        format_bytes(bytes as u64)
    }
}
