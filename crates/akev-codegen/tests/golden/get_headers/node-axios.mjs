import axios from "axios";

const response = await axios.request({
  url: "https://api.example.com/v1/items?page=2&sort=-created",
  headers: {
    "Accept": "application/json",
    "Authorization": "Bearer eyJhbGciOiJIUzI1NiJ9.e30.abc",
    "User-Agent": "Akev/1.0",
    "X-Request-Id": "42"
  }
});

console.log(response.status);
console.log(response.data);