package main

import (
	"fmt"
	"io"
	"net/http"
	"strings"
)

func main() {
	body := strings.NewReader("user=ann%40example.com&password=p%40ss+w0rd%21&remember=on")

	req, err := http.NewRequest(http.MethodPost, "https://example.com/login", body)
	if err != nil {
		panic(err)
	}
	req.Header.Add("Content-Type", "application/x-www-form-urlencoded")
	req.Header.Add("Cookie", "session=abc123; theme=dark")
	req.Header.Add("Origin", "https://example.com")

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