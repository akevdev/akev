package main

import (
	"fmt"
	"io"
	"net/http"
	"strings"
)

func main() {
	body := strings.NewReader("Line 1: it's \"quoted\" and 'single'\nLine 2: $HOME ${HOME} `whoami` $(id) %PATH% 100% #{x} #@y !!\n\tTabbed: back\\slash \\n \\\\ \"\"\" ''' ``` --%\nUnicode: héllo — 日本語 🚀 ‘smart’ “quotes”\n'@ at line start, and a trailing backslash \\")

	req, err := http.NewRequest(http.MethodPatch, "https://example.com/api/notes/1?q=a%20b&x=$y&z='\"", body)
	if err != nil {
		panic(err)
	}
	req.Header.Add("Content-Type", "text/plain; charset=utf-8")
	req.Header.Add("X-Quote", "it's \"quoted\" `cmd` $HOME %PATH% \\back\\")
	req.Header.Add("X-Unicode", "héllo 日本 ✓")
	req.Header.Add("X-Eq", "=starts-with-equals")

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