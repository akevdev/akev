import axios from "axios";

const response = await axios.request({
  method: "post",
  url: "https://api.example.com/v1/users",
  headers: {
    "Content-Type": "application/json",
    "Accept": "application/json"
  },
  data: {
    name: "Ann O'Neil",
    age: 34,
    admin: false,
    tags: ["a", "b"],
    address: {
      city: "Zürich",
      zip: null
    },
    score: 9.5,
    meta: {}
  }
});

console.log(response.status);
console.log(response.data);