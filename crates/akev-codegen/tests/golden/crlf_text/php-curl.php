<?php

$curl = curl_init();

curl_setopt_array($curl, [
    CURLOPT_URL => 'https://example.com/upload',
    CURLOPT_RETURNTRANSFER => true,
    CURLOPT_CUSTOMREQUEST => 'POST',
    CURLOPT_HTTPHEADER => [
        'Content-Type: multipart/form-data; boundary=XyZ',
    ],
    CURLOPT_POSTFIELDS => "--XyZ\r\nContent-Disposition: form-data; name=\"note\"\r\n\r\nhi \e[1m\"there\"\r\n--XyZ--\r\n",
]);

$response = curl_exec($curl);

if ($response === false) {
    echo 'cURL error: ' . curl_error($curl) . PHP_EOL;
} else {
    echo curl_getinfo($curl, CURLINFO_RESPONSE_CODE) . PHP_EOL;
    echo $response . PHP_EOL;
}