// Cargo.toml:
// [dependencies]
// reqwest = "0.12"
// tokio = { version = "1", features = ["full"] }

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = reqwest::Client::new();
    let response = client
        .get("https://api.example.com/v1/items?page=2&sort=-created")
        .header("Accept", "application/json")
        .header("Authorization", "Bearer eyJhbGciOiJIUzI1NiJ9.e30.abc")
        .header("User-Agent", "Akev/1.0")
        .header("X-Request-Id", "42")
        .send()
        .await?;

    println!("{}", response.status());
    println!("{}", response.text().await?);

    Ok(())
}