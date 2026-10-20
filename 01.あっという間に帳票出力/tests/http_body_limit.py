"""起動済みサンプルの実HTTP回帰試験: python http_body_limit.py http://127.0.0.1:8080"""
import json,sys,urllib.request,urllib.error
base=sys.argv[1].rstrip('/')
with urllib.request.urlopen(base+'/print-data') as response:data=json.load(response)
data.setdefault('ExtensionData',{}).setdefault('ReportsWeb',{})['padding']=''
for size in [2*1024**2+1,64*1024**2,64*1024**2+1,0]:
 data['ExtensionData']['ReportsWeb']['padding']=''
 body=json.dumps(data,separators=(',',':'),ensure_ascii=False).encode()
 if size:
  data['ExtensionData']['ReportsWeb']['padding']='x'*(size-len(body))
  body=json.dumps(data,separators=(',',':'),ensure_ascii=False).encode();assert len(body)==size
 try:
  with urllib.request.urlopen(urllib.request.Request(base+'/pdf',body,{'Content-Type':'application/json'}),timeout=180) as response:status=response.status;pdf=response.read()
 except urllib.error.HTTPError as error:status=error.code;pdf=error.read()
 expected=413 if size>64*1024**2 else 200
 assert status==expected,(size,status,expected,pdf[:200])
 if status==200:assert pdf.startswith(b'%PDF-')
 print(json.dumps(dict(bytes=len(body),status=status,expected=expected)))
