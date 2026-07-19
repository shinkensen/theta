use std::time::SystemTime;
pub fn get_time_now() -> u64 {
    let now = SystemTime::now();
    let secs: u64 = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_err(|e| e.to_string())
        .map(|d| d.as_secs())
        .expect("Getting the current system time failed"); //this is like the template litteral of rust
    secs
}
