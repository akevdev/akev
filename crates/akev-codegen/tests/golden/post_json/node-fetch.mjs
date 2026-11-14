const response = await fetch("https://api.example.com/v1/users", {
  method: "POST",
  headers: {
    "Content-Type": "application/json",
    "Accept": "application/json"
  },
  body: JSON.stringify({
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
  })
});

console.log(response.status);
console.log(await response.text());