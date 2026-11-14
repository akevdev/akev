echo 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJ//4iJ1w=' | base64 --decode | curl 'https://upload.example.com/files/logo.png' \
  -X PUT \
  -H 'Content-Type: image/png' \
  -H 'X-Upload-Token: t0k3n' \
  --data-binary @-