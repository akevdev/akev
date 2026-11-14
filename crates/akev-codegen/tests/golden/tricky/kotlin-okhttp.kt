import okhttp3.Headers
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody

fun main() {
    val client = OkHttpClient()

    val body = "Line 1: it's \"quoted\" and 'single'\nLine 2: \$HOME \${HOME} `whoami` \$(id) %PATH% 100% #{x} #@y !!\n\tTabbed: back\\slash \\n \\\\ \"\"\" ''' ``` --%\nUnicode: héllo — 日本語 🚀 ‘smart’ “quotes”\n'@ at line start, and a trailing backslash \\".toRequestBody("text/plain; charset=utf-8".toMediaType())

    val headers = Headers.Builder()
        .add("X-Quote", "it's \"quoted\" `cmd` \$HOME %PATH% \\back\\")
        .addUnsafeNonAscii("X-Unicode", "héllo 日本 ✓")
        .add("X-Eq", "=starts-with-equals")
        .build()

    val request = Request.Builder()
        .url("https://example.com/api/notes/1?q=a%20b&x=\$y&z='\"")
        .patch(body)
        .headers(headers)
        .build()

    client.newCall(request).execute().use { response ->
        println(response.code)
        println(response.body?.string())
    }
}