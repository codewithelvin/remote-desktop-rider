# Writes macOS's auto-login password file (/etc/kcpassword) for the test VM:
# the password XORed with Apple's fixed key, padded to a multiple of 12.
#   echo <password> | sudo python3 kcpassword.py
import sys
key = [0x7D, 0x89, 0x52, 0x23, 0xD2, 0xBC, 0xDD, 0xEA, 0xA3, 0xB9, 0x1F]
pw = sys.stdin.readline().rstrip("\n").encode()
data = pw + b"\0" * (12 - len(pw) % 12)
out = bytes(b ^ key[i % len(key)] for i, b in enumerate(data))
with open("/etc/kcpassword", "wb") as f:
    f.write(out)
import os
os.chmod("/etc/kcpassword", 0o600)
print("kcpassword written")
