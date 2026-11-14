curl 'https://example.com/upload' \
  -H 'Content-Type: multipart/form-data; boundary=XyZ' \
  --data-raw $'--XyZ\r\nContent-Disposition: form-data; name="note"\r\n\r\nhi \x1B[1m"there"\r\n--XyZ--\r\n'