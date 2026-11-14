wget --quiet --output-document=- \
  --method=POST \
  --header='Content-Type: multipart/form-data; boundary=XyZ' \
  --body-data=$'--XyZ\r\nContent-Disposition: form-data; name="note"\r\n\r\nhi \x1B[1m"there"\r\n--XyZ--\r\n' \
  'https://example.com/upload'