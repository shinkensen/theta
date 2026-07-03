// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    println!("{}", theta_lib::temp::get_time_now());
    //theta_lib::run()
}
