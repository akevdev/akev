wget --quiet --output-document=- \
  --method=POST \
  --header='Content-Type: application/x-www-form-urlencoded' \
  --header='Cookie: session=abc123; theme=dark' \
  --header='Origin: https://example.com' \
  --body-data='user=ann%40example.com&password=p%40ss+w0rd%21&remember=on' \
  'https://example.com/login'