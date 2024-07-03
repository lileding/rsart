mod art;

fn main() {
    art::CONTEXT.set(art::Runtime::new().unwrap());
    art::block_on(async_main());
}

async fn async_main() {
    println!("task 1");
    art::spawn(async { println!("task 2"); });
    art::sleep(std::time::Duration::from_secs(1)).await;
    art::spawn(async {
        art::sleep(std::time::Duration::from_secs(3)).await;
        println!("task 3");
    });
    println!("task 1 complete");
}

