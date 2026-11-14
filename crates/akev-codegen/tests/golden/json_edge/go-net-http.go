package main

import (
	"fmt"
	"io"
	"net/http"
	"strings"
)

func main() {
	body := strings.NewReader(`{
  "__proto__": {"polluted": true},
  "0": "zero",
  "big": 9007199254740993,
  "text": "line\nbreak \"q\" $x #{y} \u00e9",
  "empty": [],
  "nested": [[1, 2], {"a": null}],
  "neg": -1.5e-7
}`)

	req, err := http.NewRequest(http.MethodPost, "https://example.com/graphql", body)
	if err != nil {
		panic(err)
	}
	req.Header.Add("Content-Type", "application/json; charset=utf-8")

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