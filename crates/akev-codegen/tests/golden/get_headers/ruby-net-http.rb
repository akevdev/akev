require 'net/http'

uri = URI('https://api.example.com/v1/items?page=2&sort=-created')

request = Net::HTTP::Get.new(uri)
request['Accept'] = 'application/json'
request['Authorization'] = 'Bearer eyJhbGciOiJIUzI1NiJ9.e30.abc'
request['User-Agent'] = 'Akev/1.0'
request['X-Request-Id'] = '42'

response = Net::HTTP.start(uri.hostname, uri.port, use_ssl: uri.scheme == 'https') do |http|
  http.request(request)
end

puts response.code
puts response.body