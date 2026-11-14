$headers = @{
    'Host' = 'example.com'
    'X-Dup' = 'one, two'
    'X-Empty' = ''
}
$body = 'plain body without a content type'

$response = Invoke-WebRequest -Uri 'http://cache.local:8080/a/../b/[1]?x={y}' `
    -CustomMethod 'purge' `
    -Headers $headers `
    -Body $body

$response.StatusCode
$response.Content