const response = await fetch("https://upload.example.com/files/logo.png", {
  method: "PUT",
  headers: {
    "Content-Type": "image/png",
    "X-Upload-Token": "t0k3n"
  },
  body: Uint8Array.from(atob("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJ//4iJ1w="), (c) => c.charCodeAt(0))
});

console.log(response.status);
console.log(await response.text());