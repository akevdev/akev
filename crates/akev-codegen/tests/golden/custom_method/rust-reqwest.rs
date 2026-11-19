// Cargo.toml:
// [dependencies]
// reqwest = "0.13"
// tokio = { version = "1", features = ["full"] }

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let body = "plain body without a content type";
    let client = reqwest::Client::new();
    let response = client
        .request(
            reqwest::Method::from_bytes(b"purge")?,
            "http://cache.local:8080/a/../b/[1]?x={y}",
        )
        .header("Host", "example.com")
        .header("X-Dup", "one")
        .header("x-dup", "two")
        .header("X-Empty", "")
        .body(body)
        .send()
        .await?;

    println!("{}", response.status());
    println!("{}", response.text().await?);

    Ok(())
}