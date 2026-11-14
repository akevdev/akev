printf '%s' 'user=ann%40example.com&password=p%40ss+w0rd%21&remember=on' | http POST 'https://example.com/login' \
  'Content-Type:application/x-www-form-urlencoded' \
  'Cookie:session=abc123; theme=dark' \
  'Origin:https://example.com'