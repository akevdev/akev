wget --quiet --output-document=- \
  --header='Accept: application/json' \
  --header='Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.e30.abc' \
  --header='User-Agent: Akev/1.0' \
  --header='X-Request-Id: 42' \
  'https://api.example.com/v1/items?page=2&sort=-created'