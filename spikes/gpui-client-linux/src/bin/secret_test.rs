#[cfg(target_os = "linux")]
fn main() -> anyhow::Result<()> {
    use std::time::Instant;

    let attributes = vec![
        ("application", "junto-gpui-spike"),
        ("purpose", "secret-storage-test"),
    ];
    let secret = format!("spike-secret-{}", std::process::id());

    futures::executor::block_on(async {
        let started = Instant::now();
        let step = |name: &str| println!("[{:7.3} s] {name}", started.elapsed().as_secs_f64());

        let keyring = oo7::Keyring::new().await?;
        step(match &keyring {
            oo7::Keyring::DBus(_) => "backend: Secret Service over D-Bus",
            oo7::Keyring::File(_) => "backend: key file through the secret portal",
        });
        println!("collection locked before unlock: {}", keyring.is_locked().await?);
        keyring.unlock().await?;
        step("unlocked");

        keyring
            .create_item("Junto gpui spike test secret", &attributes, secret.as_bytes(), true)
            .await?;
        step("stored");

        let items = keyring.search_items(&attributes).await?;
        println!("items found: {}", items.len());
        for item in &items {
            item.unlock().await?;
            let read = item.secret().await?;
            println!(
                "read back: label={:?} matches={}",
                item.label().await?,
                read.as_bytes() == secret.as_bytes()
            );
        }
        step("read");

        keyring.delete(&attributes).await?;
        let left = keyring.search_items(&attributes).await?.len();
        step("deleted");
        println!("items left after delete: {left}");
        anyhow::Ok(())
    })
}

#[cfg(not(target_os = "linux"))]
fn main() {}
