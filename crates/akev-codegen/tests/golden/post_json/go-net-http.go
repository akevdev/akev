package main

import (
	"fmt"
	"io"
	"net/http"
	"strings"
)

func main() {
	body := strings.NewReader(`{"name":"Ann O'Neil","age":34,"admin":false,"tags":["a","b"],"address":{"city":"Zürich","zip":null},"score":9.5,"meta":{}}`)

	req, err := http.NewRequest(http.MethodPost, "https://api.example.com/v1/users", body)
	if err != nil {
		panic(err)
	}
	req.Header.Add("Content-Type", "application/json")
	req.Header.Add("Accept", "application/json")

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