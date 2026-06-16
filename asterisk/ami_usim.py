import asyncio
from panoramisk import Manager

from smartcard.System import readers
from smartcard.util import toHexString,toBytes

import configparser
import sys
import time

def make_connection_index(reader_index):
    r = readers()
    connection = r[reader_index].createConnection()
    try:
        connection.connect()
    except Exception:
        return None
    # Select EF.DIR
    data, sw1, sw2 = connection.transmit(toBytes('00a40004022f0000'))
    if sw1 != 97:
        print("Failed to select EF.DIR")
        return None
    data, sw1, sw2 = connection.transmit(toBytes('00C00000') + [sw2])
    result = toHexString(data).replace(" ", "")
    record_length = data[7]
    # Read first AID
    data, sw1, sw2 = connection.transmit(toBytes('00b20104') + [record_length])
    if sw1 != 144:
        print("Failed to get AID")
        return None
    result = toHexString(data).replace(" ", "")
    aid_length = data[3]
    aid = result[8:(8 + aid_length * 2)]
    print(f"Using aid={aid}")
    # Select AID
    data, sw1, sw2 = connection.transmit(toBytes('00a40404') + [aid_length] + toBytes(aid))
    if sw1 != 97:
        print("Failed to select AID")
        return None
    return connection


def swap_nibbles(s: Hexstr) -> hexstr:
    """swap the nibbles in a hex string"""
    return ''.join([x+y for x, y in zip(s[1::2], s[0::2])])


def dec_imsi(ef: Hexstr) -> Optional[str]:
    """Converts an EF value to the IMSI string representation"""
    if len(ef) < 4:
        return None
    l = int(ef[0:2], 16) * 2		# Length of the IMSI string
    l = l - 1						# Encoded length byte includes oe nibble
    swapped = swap_nibbles(ef[2:]).rstrip('f')
    if len(swapped) < 1:
        return None
    oe = (int(swapped[0]) >> 3) & 1  # Odd (1) / Even (0)
    if not oe:
        # if even, only half of last byte was used
        l = l-1
    if l != len(swapped) - 1:
        return None
    imsi = swapped[1:]
    return imsi


def make_connection_name(reader_name):
    if reader_name.startswith('imsi:'):
        target_imsi = reader_name[5:]
        l = len(readers())
        for idx in range(l):
            connection = make_connection_index(idx)
            if connection is None:
                continue
            data, sw1, sw2 = connection.transmit(toBytes('00a40004026f07'))
            if sw1 != 0x61:
                print(f"Unexpected Select IMSI result: {sw1:x}, {sw2:x}")
                continue
            data, sw1, sw2 = connection.transmit(toBytes('00b0000009'))
            if (sw1, sw2) != (0x90, 0x00):
                print(f"Unexpected Read binary IMSI result: {sw1:x}, {sw2:x}")
                continue
            imsi = dec_imsi(bytes(data).hex())
            print(f"Found IMSI {imsi}")
            if imsi == target_imsi:
                print(f"Found target imsi on reader {idx}")
                return connection
        print(f"IMSI {target_imsi} not found")
        return None
    return make_connection_index(int(reader_name))


# Do USIM authentication, return RES/CK/IK vector || AUTS || None
def read_res_ck_ik(reader_index, rand, autn):
    res = None
    ck = None
    ik = None
    auts = None
    connection = make_connection_name(reader_index)
    if connection is None:
        return res, ck, ik, auts
    # Authenticate
    data, sw1, sw2 = connection.transmit(toBytes('008800812210' + rand.upper() + '10' + autn.upper()))
    if sw1 == 97:
        data, sw1, sw2 = connection.transmit(toBytes('00C00000') + [sw2])
        result = toHexString(data).replace(" ", "")
        print(f"Authentication result={result}")
        rc = result[0:2]
        if rc == 'DB':
            res_length = data[1];
            res = result[4:(4 + res_length * 2)]
            ck_length = data[2 + res_length]
            ck = result[(6 + res_length * 2):(6 + res_length * 2 + ck_length * 2)]
            ik_length = data[2 + res_length + 1 + ck_length]
            ik = result[(8 + res_length * 2 + ck_length * 2):(8 + res_length * 2 + ck_length * 2 + ik_length * 2)]
        elif rc == 'DC':
            auts = result[4:32]
    else :
        print(f"Authentication failed.")

    return res, ck, ik, auts


def main():
    loop = asyncio.new_event_loop()
    asyncio.set_event_loop(loop)

    if len(sys.argv) != 2:
        print(f"Usage: python {sys.argv[0]} <ini-file>")
        sys.exit(1)

    # Parse configuration file
    config = configparser.ConfigParser()
    config.read(sys.argv[1])
    cfg_endpoint = config.sections()[0]
    cfg_reader = config.get(cfg_endpoint, 'reader')
    cfg_host = config.get(cfg_endpoint, 'host')
    cfg_username = config.get(cfg_endpoint, 'username')
    cfg_secret = config.get(cfg_endpoint, 'secret')
    print("Endpoint name: " + cfg_endpoint)
    print("Card reader: " + cfg_reader)
    print("AMI Host: " + cfg_host)
    print("AMI User: " + cfg_username)
    print("AMI Pass: " + cfg_secret)

    # Create AMI manager
    manager = Manager(loop=asyncio.get_event_loop(),
                      host=cfg_host,
                      username=cfg_username,
                      secret=cfg_secret)

    # Register, if Asterisk reports that it is fully booted.
    @manager.register_event('FullyBooted')
    def callback(manager, message):
        print("Asterisk is ready, trigger registration...")
        manager.send_action({'Action': 'PJSIPRegister', 'Registration': cfg_endpoint})
        print("Registering sent")
#        manager.send_action({'Action': 'Events', 'EventMask': 'on'})
#        print("Enable events")

    # Upon Authentication request, ask SIM to authenticate and return result
    @manager.register_event('AuthRequest')
    def callback(manager, message):
        algo = message.Algorithm
        rand = message.RAND
        autn = message.AUTN
        print(f"AuthRequest received: Algorithm={algo}, RAND={rand}, AUTN={autn}")
        res,ck,ik,auts = read_res_ck_ik(cfg_reader, rand, autn)
        if res is not None:
            manager.send_action({'Action': 'AuthResponse', 'Registration': cfg_endpoint, "RES": res, "CK": ck, "IK": ik})
        elif auts is not None:
            manager.send_action({'Action': 'AuthResponse', 'Registration': cfg_endpoint, "AUTS": auts})
        else:
            manager.send_action({'Action': 'AuthResponse', 'Registration': cfg_endpoint})
        print(f"AuthResponse sent: RES={res}, CK={ck}, IK={ik}, AUTS={auts}")

    # Upon Event: newchannel
    @manager.register_event('Newchannel')
    def callback(manager, message):
        context = message.Context
        channel = message.Channel
        print(f"New channel for context received: Context={context}, Channel={channel}")
        time.sleep(0.5)
        if (context == cfg_endpoint):
            manager.send_action({'Action': 'DedicatedBearerStatus', 'Channel': channel, 'Status': 'Up'})
            print(f"DedicatedBearerStatus sent: Channel={channel}")

    # Connect AMI manager and run main loop
    manager.connect()
    try:
        manager.loop.run_forever()
    except KeyboardInterrupt:
        manager.loop.close()


if __name__ == '__main__':
    main()
