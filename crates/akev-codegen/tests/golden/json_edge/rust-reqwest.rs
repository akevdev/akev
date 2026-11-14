// Cargo.toml:
// [dependencies]
// reqwest = "0.12"
// tokio = { version = "1", features = ["full"] }

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let body = r#"{
  "__proto__": {"polluted": true},
  "0": "zero",
  "big": 9007199254740993,
  "text": "line\nbreak \"q\" $x #{y} \u00e9",
  "empty": [],
  "nested": [[1, 2], {"a": null}],
  "neg": -1.5e-7
}"#;
    let client = reqwest::Client::new();
    let response = client
        .post("https://example.com/graphql")
        .header("Content-Type", "application/json; charset=utf-8")
        .body(body)
        .send()
        .await?;

    println!("{}", response.status());
    println!("{}", response.text().await?);

    Ok(())
}