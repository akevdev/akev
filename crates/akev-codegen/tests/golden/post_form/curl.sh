curl 'https://example.com/login' \
  -H 'Content-Type: application/x-www-form-urlencoded' \
  -H 'Cookie: session=abc123; theme=dark' \
  -H 'Origin: https://example.com' \
  --data-raw 'user=ann%40example.com' \
  --data-raw 'password=p%40ss+w0rd%21' \
  --data-raw 'remember=on'