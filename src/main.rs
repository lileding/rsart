mod art;

use log::info;

fn main() {
    env_logger::init();
    art::CONTEXT.set(art::Runtime::new().unwrap());
    info!("main return {}", art::block_on(async_main()));
}

async fn async_main() -> i32 {
    info!("task 0 begin");
    let rv = art::spawn(async { info!("task 1 running"); 123 });
    info!("task 1 complete with {}", rv.await);
    art::sleep(std::time::Duration::from_secs(1)).await;
    art::spawn(async {
        info!("task 2 begin sleep");
        art::sleep(std::time::Duration::from_secs(3)).await;
        info!("task 2 complete");
    });
    info!("task 0 complete");
    1
}

