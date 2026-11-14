package main

import (
	"fmt"
	"io"
	"net/http"
	"strings"
)

func main() {
	body := strings.NewReader("--XyZ\r\nContent-Disposition: form-data; name=\"note\"\r\n\r\nhi \x1B[1m\"there\"\r\n--XyZ--\r\n")

	req, err := http.NewRequest(http.MethodPost, "https://example.com/upload", body)
	if err != nil {
		panic(err)
	}
	req.Header.Add("Content-Type", "multipart/form-data; boundary=XyZ")

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