//! Prints every device `bluetooth::read_devices` finds, one menu line each.

use std::time::SystemTime;

fn main() -> anyhow::Result<()> {
    let now = SystemTime::now();
    for device in bluetooth::read_devices()? {
        println!("{}", ui_model::menu_line(&device, now));
    }
    Ok(())
}
