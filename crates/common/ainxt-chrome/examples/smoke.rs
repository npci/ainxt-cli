//! Smoke test: launch Chrome, open a page, print its accessibility outline.
//!
//!   cargo run -p ainxt-chrome --example smoke -- https://example.com

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "https://example.com".to_owned());

    let config = ainxt_chrome::LaunchConfig {
        headless: std::env::var("SMOKE_HEADLESS").is_ok(),
        port: 9222,
        ..Default::default()
    };
    println!("profile: {}", config.user_data_dir.display());

    let browser = ainxt_chrome::Browser::launch(config).await?;
    println!("devtools port: {}", browser.port());

    let page = browser.open(&url).await?;
    let info = page.info().await?;
    println!("url:   {}", info.url);
    println!("title: {}", info.title);
    println!("--- accessibility tree ---");
    println!("{}", page.read_accessibility_tree(4_000).await?);
    Ok(())
}
