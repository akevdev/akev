require 'net/http'

uri = URI('https://example.com/upload')

request = Net::HTTP::Post.new(uri)
request['Content-Type'] = 'multipart/form-data; boundary=XyZ'
request.body = "--XyZ\r
Content-Disposition: form-data; name=\"note\"\r
\r
hi \e[1m\"there\"\r
--XyZ--\r
"

response = Net::HTTP.start(uri.hostname, uri.port, use_ssl: uri.scheme == 'https') do |http|
  http.request(request)
end

puts response.code
puts response.body