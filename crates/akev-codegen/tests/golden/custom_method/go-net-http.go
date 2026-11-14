package main

import (
	"fmt"
	"io"
	"net/http"
	"strings"
)

func main() {
	body := strings.NewReader("plain body without a content type")

	req, err := http.NewRequest("purge", "http://cache.local:8080/a/../b/[1]?x={y}", body)
	if err != nil {
		panic(err)
	}
	req.Header.Add("X-Dup", "one")
	req.Header.Add("x-dup", "two")
	req.Header.Add("X-Empty", "")
	req.Host = "example.com"

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