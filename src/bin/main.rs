use std::io::{Result, Error};
use std::time::Duration;
use log::info;

#[rsart::main]
async fn main() -> Result<()> {
    let args = std::env::args().collect::<Vec<String>>();
    info!("task 0 begin");
    let rv = rsart::spawn(async move {
        info!("task 1 running");
        rsart::sleep(Duration::from_secs(3)).await;
        args.get(1).unwrap_or(&String::default()).to_owned()
    });
    rsart::spawn(async {
        info!("task 2 begin sleep");
        rsart::sleep(Duration::from_secs(1)).await;
        info!("task 2 complete");
    });
    info!("join task 1");
    let rv = rv.await;
    info!("task 1 complete with {:?}", &rv);
    if rv.len() > 3 {
        return Err(Error::other(rv));
    }
    info!("task 0 complete");
    Ok(())
}
