curl "https://api.example.com/v1/items?page=2&sort=-created" ^
  -H "Accept: application/json" ^
  -H "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.e30.abc" ^
  -H "User-Agent: Akev/1.0" ^
  -H "X-Request-Id: 42" ^
  --compressed