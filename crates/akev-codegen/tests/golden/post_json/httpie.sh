printf '%s' '{"name":"Ann O'\''Neil","age":34,"admin":false,"tags":["a","b"],"address":{"city":"Zürich","zip":null},"score":9.5,"meta":{}}' | http POST 'https://api.example.com/v1/users' \
  'Content-Type:application/json' \
  'Accept:application/json'