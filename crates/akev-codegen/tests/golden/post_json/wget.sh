wget --quiet --output-document=- \
  --method=POST \
  --header='Content-Type: application/json' \
  --header='Accept: application/json' \
  --body-data='{"name":"Ann O'\''Neil","age":34,"admin":false,"tags":["a","b"],"address":{"city":"Zürich","zip":null},"score":9.5,"meta":{}}' \
  'https://api.example.com/v1/users'