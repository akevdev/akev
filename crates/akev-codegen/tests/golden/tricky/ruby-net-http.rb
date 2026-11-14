require 'net/http'

uri = URI('https://example.com/api/notes/1?q=a%20b&x=$y&z=\'"')

request = Net::HTTP::Patch.new(uri)
request['Content-Type'] = 'text/plain; charset=utf-8'
request['X-Quote'] = 'it\'s "quoted" `cmd` $HOME %PATH% \\back\\'
request['X-Unicode'] = 'héllo 日本 ✓'
request['X-Eq'] = '=starts-with-equals'
request.body = 'Line 1: it\'s "quoted" and \'single\'
Line 2: $HOME ${HOME} `whoami` $(id) %PATH% 100% #{x} #@y !!
	Tabbed: back\\slash \\n \\\\ """ \'\'\' ``` --%
Unicode: héllo — 日本語 🚀 ‘smart’ “quotes”
\'@ at line start, and a trailing backslash \\'

response = Net::HTTP.start(uri.hostname, uri.port, use_ssl: uri.scheme == 'https') do |http|
  http.request(request)
end

puts response.code
puts response.body