echo 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJ//4iJ1w=' | base64 --decode | http PUT 'https://upload.example.com/files/logo.png' \
  'Content-Type:image/png' \
  'X-Upload-Token:t0k3n'