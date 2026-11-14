require 'net/http'

uri = URI('https://upload.example.com/files/logo.png')

request = Net::HTTP::Put.new(uri)
request['Content-Type'] = 'image/png'
request['X-Upload-Token'] = 't0k3n'
request.body = 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJ//4iJ1w='.unpack1('m')

response = Net::HTTP.start(uri.hostname, uri.port, use_ssl: uri.scheme == 'https') do |http|
  http.request(request)
end

puts response.code
puts response.body