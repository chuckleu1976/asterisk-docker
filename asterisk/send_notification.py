#!/usr/bin/python3

import sys
import requests
import base64

TOKEN_FILE = '/usr/local/etc/telegram_token'

requests.packages.urllib3.util.connection.HAS_IPV6 = False

if (len(sys.argv) < 3):
    print("Too few arguments")

telegramto = sys.argv[1]
body = sys.argv[2]

with open(TOKEN_FILE) as f:
    BOT_TOKEN = f.read().strip()
API_URL = 'https://api.telegram.org/bot' + BOT_TOKEN + '/'

def apiRequest(method, parameters):
    try:
        requests.get(API_URL + method, data=parameters)
    except Exception as e:
        pass
    
apiRequest("sendMessage", {'chat_id': telegramto, "text": base64.b64decode(body)})

