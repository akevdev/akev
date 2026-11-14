$headers = @{
    'Cookie' = 'session=abc123; theme=dark'
    'Origin' = 'https://example.com'
}
$body = 'user=ann%40example.com&password=p%40ss+w0rd%21&remember=on'

$response = Invoke-WebRequest -Uri 'https://example.com/login' `
    -Method Post `
    -Headers $headers `
    -ContentType 'application/x-www-form-urlencoded' `
    -Body $body

$response.StatusCode
$response.Content