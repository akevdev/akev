curl "https://api.example.com/v1/users" ^
  -H "Content-Type: application/json" ^
  -H "Accept: application/json" ^
  --data-raw "{\"name\":\"Ann O'Neil\",\"age\":34,\"admin\":false,\"tags\":[\"a\",\"b\"],\"address\":{\"city\":\"Zürich\",\"zip\":null},\"score\":9.5,\"meta\":{}}"