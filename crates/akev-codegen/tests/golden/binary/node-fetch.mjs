const response = await fetch("https://upload.example.com/files/logo.png", {
  method: "PUT",
  headers: {
    "Content-Type": "image/png",
    "X-Upload-Token": "t0k3n"
  },
  body: Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJ//4iJ1w=", "base64")
});

console.log(response.status);
console.log(await response.text());