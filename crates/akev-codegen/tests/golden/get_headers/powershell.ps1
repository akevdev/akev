$headers = @{
    'Accept' = 'application/json'
    'Authorization' = 'Bearer eyJhbGciOiJIUzI1NiJ9.e30.abc'
    'X-Request-Id' = '42'
}

$response = Invoke-WebRequest -Uri 'https://api.example.com/v1/items?page=2&sort=-created' `
    -Headers $headers `
    -UserAgent 'Akev/1.0'

$response.StatusCode
$response.Content