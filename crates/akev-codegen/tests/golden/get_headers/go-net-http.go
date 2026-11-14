package main

import (
	"fmt"
	"io"
	"net/http"
)

func main() {
	req, err := http.NewRequest(http.MethodGet, "https://api.example.com/v1/items?page=2&sort=-created", nil)
	if err != nil {
		panic(err)
	}
	req.Header.Add("Accept", "application/json")
	req.Header.Add("Authorization", "Bearer eyJhbGciOiJIUzI1NiJ9.e30.abc")
	req.Header.Add("User-Agent", "Akev/1.0")
	req.Header.Add("X-Request-Id", "42")

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