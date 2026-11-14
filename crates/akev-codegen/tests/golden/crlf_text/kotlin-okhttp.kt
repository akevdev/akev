import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody

fun main() {
    val client = OkHttpClient()

    val body = "--XyZ\r\nContent-Disposition: form-data; name=\"note\"\r\n\r\nhi \u001B[1m\"there\"\r\n--XyZ--\r\n".toRequestBody("multipart/form-data; boundary=XyZ".toMediaType())

    val request = Request.Builder()
        .url("https://example.com/upload")
        .post(body)
        .build()

    client.newCall(request).execute().use { response ->
        println(response.code)
        println(response.body?.string())
    }
}