$body = "--XyZ`r`nContent-Disposition: form-data; name=`"note`"`r`n`r`nhi `u{1B}[1m`"there`"`r`n--XyZ--`r`n"

$response = Invoke-WebRequest -Uri 'https://example.com/upload' `
    -Method Post `
    -ContentType 'multipart/form-data; boundary=XyZ' `
    -Body $body

$response.StatusCode
$response.Content