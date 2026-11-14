package main

import (
	"bytes"
	"encoding/base64"
	"fmt"
	"io"
	"net/http"
)

func main() {
	data, err := base64.StdEncoding.DecodeString("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJ//4iJ1w=")
	if err != nil {
		panic(err)
	}
	body := bytes.NewReader(data)

	req, err := http.NewRequest(http.MethodPut, "https://upload.example.com/files/logo.png", body)
	if err != nil {
		panic(err)
	}
	req.Header.Add("Content-Type", "image/png")
	req.Header.Add("X-Upload-Token", "t0k3n")

	res, err := http.DefaultClient.Do(req)
	if err != nil {
		panic(err)
	}
	defer res.Body.Close()

	resBody, err := io.ReadAll(res.Body)
	if err != nil {
		panic(err)
	}

	fmt.Println(res.Status)
	fmt.Println(string(resBody))
}