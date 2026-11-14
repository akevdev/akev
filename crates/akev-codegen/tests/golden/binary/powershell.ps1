$headers = @{
    'X-Upload-Token' = 't0k3n'
}
$body = [System.Convert]::FromBase64String('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJ//4iJ1w=')

$response = Invoke-WebRequest -Uri 'https://upload.example.com/files/logo.png' `
    -Method Put `
    -Headers $headers `
    -ContentType 'image/png' `
    -Body $body

$response.StatusCode
$response.Content