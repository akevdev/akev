import axios from "axios";

const response = await axios.request({
  method: "put",
  url: "https://upload.example.com/files/logo.png",
  headers: {
    "Content-Type": "image/png",
    "X-Upload-Token": "t0k3n"
  },
  data: Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJ//4iJ1w=", "base64")
});

console.log(response.status);
console.log(response.data);