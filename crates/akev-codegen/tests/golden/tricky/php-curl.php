<?php

$curl = curl_init();

curl_setopt_array($curl, [
    CURLOPT_URL => 'https://example.com/api/notes/1?q=a%20b&x=$y&z=\'"',
    CURLOPT_RETURNTRANSFER => true,
    CURLOPT_CUSTOMREQUEST => 'PATCH',
    CURLOPT_HTTPHEADER => [
        'Content-Type: text/plain; charset=utf-8',
        'X-Quote: it\'s "quoted" `cmd` $HOME %PATH% \\back\\',
        'X-Unicode: héllo 日本 ✓',
        'X-Eq: =starts-with-equals',
    ],
    CURLOPT_POSTFIELDS => 'Line 1: it\'s "quoted" and \'single\'
Line 2: $HOME ${HOME} `whoami` $(id) %PATH% 100% #{x} #@y !!
	Tabbed: back\\slash \\n \\\\ """ \'\'\' ``` --%
Unicode: héllo — 日本語 🚀 ‘smart’ “quotes”
\'@ at line start, and a trailing backslash \\',
]);

$response = curl_exec($curl);

if ($response === false) {
    echo 'cURL error: ' . curl_error($curl) . PHP_EOL;
} else {
    echo curl_getinfo($curl, CURLINFO_RESPONSE_CODE) . PHP_EOL;
    echo $response . PHP_EOL;
}