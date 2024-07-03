mod art;

fn main() {
    art::CONTEXT.set(art::Runtime::new().unwrap());
    art::block_on(async {
        println!("task 1");
        art::spawn(async { println!("task 2"); });
        art::sleep(std::time::Duration::from_secs(1)).unwrap().await;
        art::spawn(async {
            art::sleep(std::time::Duration::from_secs(3)).unwrap().await;
            println!("task 3");
        });
        println!("task 1 complete");
    });
}

