body_file=$(mktemp)
echo 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJ//4iJ1w=' | base64 --decode > "$body_file"
wget --quiet --output-document=- \
  --method=PUT \
  --header='Content-Type: image/png' \
  --header='X-Upload-Token: t0k3n' \
  --body-file="$body_file" \
  'https://upload.example.com/files/logo.png'