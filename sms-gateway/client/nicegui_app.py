from nicegui import ui, app, run
import requests
from requests.auth import HTTPBasicAuth
import datetime
import os
from pathlib import Path

CLIENT_DIR = Path(__file__).parent.resolve()

DEFAULT_URL  = os.environ.get('SMSGATE_URL',  'http://127.0.0.1:8080')
DEFAULT_USER = os.environ.get('SMSGATE_USER', 'admin')
DEFAULT_PASS = os.environ.get('SMSGATE_PASS', '123456')

app.add_static_files('/client_assets', str(CLIENT_DIR))

app_state = {
    'server_url': DEFAULT_URL,
    'username':   DEFAULT_USER,
    'password':   DEFAULT_PASS,
    'is_polling': False,
}
log_lines     = []
_initial_done = False

NET_STATUS = {'0': '未注册', '1': '本地', '2': '搜索中', '3': '已拒绝', '5': '漫游'}


def get_auth():
    return HTTPBasicAuth(app_state['username'], app_state['password'])


def add_log(port_label: str, message: str, color: str = '#00cc00'):
    ts   = datetime.datetime.now().strftime('%Y-%m-%d %H:%M:%S')
    line = (
        f"<div style='white-space:nowrap;font-size:11px;'>"
        f"<span style='color:#777'>{ts}</span>&nbsp;"
        f"<span style='color:{color};font-weight:bold'>{port_label}</span>&nbsp;"
        f"<span style='color:#00cc00'>{message}</span>"
        f"</div>"
    )
    log_lines.append(line)
    if len(log_lines) > 300:
        log_lines.pop(0)
    log_html.content = ''.join(log_lines[-80:])


async def fetch_sims():
    global _initial_done
    url = f"{app_state['server_url']}/api/sims/info"
    try:
        r = await run.io_bound(requests.get, url, auth=get_auth(), timeout=15)
        if r.status_code == 200:
            data = r.json()
            rows = []
            data.sort(key=lambda s: int(''.join(filter(str.isdigit, s.get('com_port', 'COM0'))) or '0'))
            for i, s in enumerate(data, 1):
                net  = s.get('network_registration') or {}
                sig  = s.get('signal_quality') or {}
                op   = s.get('operator_info') or {}
                rssi = sig.get('rssi', 99)
                ber  = sig.get('ber',  99)
                r_d  = 0 if rssi == 99 else rssi
                b_d  = 0 if ber  == 99 else ber
                sig_str    = f"{b_d}/{r_d}/4G"
                sim_status = s.get('sim_status') or ''
                connected  = sim_status == 'READY'
                net_code   = str(net.get('status', ''))
                operator   = (op.get('operator_name') or '') if op else ''
                com        = s.get('com_port', '')
                rows.append({
                    'row_num':     i,
                    'com_port':    com,
                    'module':      (s.get('model_info') or {}).get('model', ''),
                    'signal':      sig_str,
                    'rssi_raw':    r_d,
                    'status':      '已连接' if connected else '未连接',
                    'work':        '检测' if connected else '离线',
                    'phone':       s.get('phone_number') or '',
                    'country':     '[chn]+86/中国/china' if operator else '',
                    'sms_count':   0,
                    'voice_count': 0,
                    'operator':    operator,
                    'iccid':       s.get('sim_id', ''),
                    'net_status':  NET_STATUS.get(net_code, net_code),
                    'send_ok':     0,
                    'send_fail':   0,
                })
                if not _initial_done and connected:
                    add_log(f"#{i}  {com}", f"(波特率:115200) 端口连接成功!", '#0066ff')
            sim_grid.options['rowData'] = rows
            sim_grid.update()
            status_label.set_text(f"端口(已选/所有): 0/{len(rows)},  号码: 1")
            _initial_done = True
        elif r.status_code == 401:
            add_log('错误', '认证失败 – 请检查用户名/密码', 'red')
        else:
            add_log('错误', f'HTTP {r.status_code}', 'red')
    except Exception as e:
        add_log('错误', str(e), 'red')


async def fetch_sms_data():
    url          = f"{app_state['server_url']}/api/sms"
    contacts_url = f"{app_state['server_url']}/api/contacts"
    try:
        r  = await run.io_bound(
            requests.get, url, auth=get_auth(),
            params={'page': 1, 'per_page': 1000}, timeout=10,
        )
        rc = await run.io_bound(
            requests.get, contacts_url, auth=get_auth(), timeout=10,
        )
        contact_map = {}
        if rc.status_code == 200:
            for c in rc.json():
                contact_map[c['id']] = c.get('name', c['id'])

        if r.status_code == 200:
            payload  = r.json()
            messages = payload.get('data', payload) if isinstance(payload, dict) else payload
            sent, recv = [], []
            recv_counts      = {}
            send_ok_counts   = {}
            send_fail_counts = {}
            for m in messages:
                sim_id     = m.get('sim_id', '')
                contact_id = m.get('contact_id', '')
                row = {
                    'timestamp': m.get('timestamp', ''),
                    'contact':   contact_map.get(contact_id, contact_id),
                    'sim_id':    sim_id,
                    'status':    m.get('status', ''),
                    'message':   m.get('message', ''),
                }
                if m.get('send'):
                    sent.append(row)
                    if m.get('status') == 3:
                        send_fail_counts[sim_id] = send_fail_counts.get(sim_id, 0) + 1
                    elif m.get('status') != 2:
                        send_ok_counts[sim_id] = send_ok_counts.get(sim_id, 0) + 1
                else:
                    recv.append(row)
                    recv_counts[sim_id] = recv_counts.get(sim_id, 0) + 1

            sms_grid.options['rowData']  = sent
            sms_grid.update()
            recv_grid.options['rowData'] = recv
            recv_grid.update()

            # Update sms_count / send_ok / send_fail in the port monitor grid
            if sim_grid.options.get('rowData'):
                for row in sim_grid.options['rowData']:
                    sid = row.get('iccid', '')
                    row['sms_count']  = recv_counts.get(sid, 0)
                    row['send_ok']    = send_ok_counts.get(sid, 0)
                    row['send_fail']  = send_fail_counts.get(sid, 0)
                sim_grid.update()
    except Exception as e:
        add_log('错误', str(e), 'red')


async def send_sms_action():
    sim_id  = send_sim_input.value.strip()
    contact = send_contact_input.value.strip()
    message = send_msg_input.value.strip()
    if not sim_id or not contact or not message:
        ui.notify('请填写 SIM ID、手机号码和短信内容', color='warning')
        return
    try:
        r = await run.io_bound(
            requests.post,
            f"{app_state['server_url']}/api/sms",
            auth=get_auth(),
            json={'sim_id': sim_id, 'contact': contact, 'message': message, 'new': False},
            timeout=10,
        )
        if r.status_code in (200, 201):
            ui.notify('短信已发送!', color='positive')
            add_log(sim_id[:20], f'→ {contact}: {message[:30]}', '#00cc00')
            send_msg_input.value = ''
            await fetch_sms_data()
        else:
            ui.notify(f'发送失败: HTTP {r.status_code}', color='negative')
            add_log('错误', f'发送失败 HTTP {r.status_code}', 'red')
    except Exception as e:
        add_log('错误', str(e), 'red')


async def check_health():
    try:
        r = await run.io_bound(
            requests.get, f"{app_state['server_url']}/api/check",
            auth=get_auth(), timeout=5,
        )
        if r.status_code == 204:
            add_log('系统', '服务器连接正常 (204)', '#00cc00')
            ui.notify('服务器连接正常 ✓', color='positive')
        else:
            add_log('系统', f'HTTP {r.status_code}', 'orange')
    except Exception as e:
        add_log('错误', str(e), 'red')
        ui.notify('服务器无法连接', color='negative')


async def refresh_all():
    await fetch_sims()
    await fetch_sms_data()


async def _auto_poll():
    if app_state['is_polling']:
        await refresh_all()


ui.add_css('''
    body { font-family: "Microsoft YaHei", Arial, sans-serif; }
    .ag-header            { background-color: #d0d0d0 !important; }
    .ag-header-cell-label { color: #111 !important; font-weight: bold; font-size: 11px; }
    .ag-row               { font-size: 11px; }
    .ag-row-even          { background-color: #f5f5f5; }
    .ag-row-odd           { background-color: #ffffff; }
    .ag-cell              { padding: 2px 4px !important; }
    .btn-action {
        background: #cc0000 !important;
        color: white !important;
        font-size: 13px !important;
        font-weight: bold !important;
        border-radius: 2px !important;
        min-width: 108px;
        height: 36px;
    }
    .btn-action:hover { background: #aa0000 !important; }
''')

with ui.column().classes('w-full h-screen no-wrap p-0 m-0'):

    with ui.row().classes('w-full items-center justify-between px-4 py-2 bg-red-700'):
        with ui.row().classes('items-center gap-2'):
            ui.label('SMS Gateway').classes('text-xl font-bold text-white')
            ui.label('/').classes('text-white opacity-60')
            ui.label('Dangs Modem').classes('text-base text-red-200')
        with ui.row().classes('items-center gap-1'):
            ui.input(placeholder='Server URL').bind_value(
                app_state, 'server_url'
            ).props('dense outlined dark').classes('w-44')
            ui.input(placeholder='User').bind_value(
                app_state, 'username'
            ).props('dense outlined dark').classes('w-16')
            ui.input(placeholder='Pass').bind_value(
                app_state, 'password'
            ).props('dense outlined dark type=password').classes('w-16')
            ui.button('检测', on_click=check_health).props('dense flat').classes(
                'text-white text-xs px-1 border border-white border-opacity-50'
            )
            ui.button('刷新', on_click=refresh_all).props('dense flat').classes(
                'text-white text-xs px-1 border border-white border-opacity-50'
            )

    with ui.row().classes('w-full items-center gap-2 px-3 py-2 bg-white border-b shadow-sm'):
        ui.button('连接设备 1',
                  on_click=lambda: add_log('系统', '连接设备功能待实现', '#888')
                  ).classes('btn-action')
        ui.button('断开设备 2',
                  on_click=lambda: add_log('系统', '断开设备功能待实现', '#888')
                  ).classes('btn-action')
        ui.button('USSD管理 3',
                  on_click=lambda: add_log('系统', 'USSD功能待实现', '#888')
                  ).classes('btn-action')
        ui.button('端口快查 4', on_click=refresh_all).classes('btn-action')
        ui.button('端口排序 5',
                  on_click=lambda: add_log('系统', '端口排序功能待实现', '#888')
                  ).classes('btn-action')
        ui.button('参数设置 6',
                  on_click=lambda: settings_dialog.open()
                  ).classes('btn-action')
        ui.space()
        ui.input(placeholder='搜索...').props('dense outlined clearable').classes('w-36')
        def _toggle_poll(e):
            app_state['is_polling'] = e.value
            add_log('系统', f"自动刷新 {'已开启(10s)' if e.value else '已关闭'}", '#888')
        ui.switch('自动').on('change', _toggle_poll).classes('text-xs')

    with ui.tabs().classes('w-full bg-white border-b text-sm') as main_tabs:
        tab_ports = ui.tab('端口监控')
        tab_send  = ui.tab('发送记录')
        tab_recv  = ui.tab('接收记录')

    with ui.tab_panels(main_tabs, value=tab_ports).classes('w-full flex-grow overflow-hidden bg-gray-50'):

        with ui.tab_panel(tab_ports).classes('p-1 h-full'):
            sim_grid = ui.aggrid({
                'defaultColDef': {'resizable': True, 'sortable': True, 'filter': True},
                'columnDefs': [
                    {'headerName': '#',    'field': 'row_num',    'width': 50,
                     'checkboxSelection': True, 'headerCheckboxSelection': True},
                    {'headerName': '端口',   'field': 'com_port',    'width': 70},
                    {'headerName': '模块',   'field': 'module',      'width': 80},
                    {'headerName': '信号',   'field': 'signal',      'width': 90,
                     'cellStyle': {'function': "const v=parseInt((params.value||'0').split('/')[1]);return {color:v>14?'#009900':v>0?'#cc6600':'#cc0000',fontWeight:'bold'};"}},
                    {'headerName': '状态',   'field': 'status',      'width': 72,
                     'cellStyle': {'function': "return {color:params.value==='已连接'?'#0000cc':'#cc0000'};"}},
                    {'headerName': '工作',   'field': 'work',        'width': 58,
                     'cellStyle': {'function': "return {color:'#cc0000'};"}},
                    {'headerName': '号码',   'field': 'phone',       'width': 100},
                    {'headerName': '国家',   'field': 'country',     'width': 155},
                    {'headerName': '短信',   'field': 'sms_count',   'width': 50},
                    {'headerName': '语音',   'field': 'voice_count', 'width': 50},
                    {'headerName': '运营商', 'field': 'operator',    'width': 130},
                    {'headerName': 'ICCID',  'field': 'iccid',       'width': 200},
                    {'headerName': '网络',   'field': 'net_status',  'width': 90},
                    {'headerName': '发送成功', 'field': 'send_ok',   'width': 72,
                     'cellStyle': {'function': "return {color:'#0000cc'};"}},
                    {'headerName': '发送失败', 'field': 'send_fail', 'width': 72,
                     'cellStyle': {'function': "return {color:'#cc0000'};"}},
                ],
                'rowData': [],
                'rowSelection': 'multiple',
                'animateRows': True,
            }).classes('w-full h-full')

        with ui.tab_panel(tab_send).classes('p-2 h-full flex flex-col gap-2'):
            with ui.row().classes('w-full items-end gap-2 bg-white p-2 rounded border shadow-sm'):
                send_sim_input     = ui.input('SIM ID (ICCID)').props('dense').classes('w-52')
                send_contact_input = ui.input('手机号码').props('dense').classes('w-36')
                send_msg_input     = ui.input('短信内容').props('dense').classes('flex-grow')
                ui.button('发送短信', on_click=send_sms_action).classes('btn-action')
            sms_grid = ui.aggrid({
                'defaultColDef': {'sortable': True, 'resizable': True, 'filter': True},
                'columnDefs': [
                    {'headerName': '时间',   'field': 'timestamp', 'width': 180},
                    {'headerName': '手机号', 'field': 'contact',   'width': 160},
                    {'headerName': 'ICCID',  'field': 'sim_id',    'width': 210},
                    {'headerName': '状态',   'field': 'status',    'width': 80},
                    {'headerName': '内容',   'field': 'message',   'flex': 1},
                ],
                'rowData': [],
            }).classes('w-full flex-grow')

        with ui.tab_panel(tab_recv).classes('p-1 h-full'):
            recv_grid = ui.aggrid({
                'defaultColDef': {'sortable': True, 'resizable': True, 'filter': True},
                'columnDefs': [
                    {'headerName': '时间',   'field': 'timestamp', 'width': 180},
                    {'headerName': '发件人', 'field': 'contact',   'width': 160},
                    {'headerName': 'ICCID',  'field': 'sim_id',    'width': 210},
                    {'headerName': '内容',   'field': 'message',   'flex': 1},
                ],
                'rowData': [],
            }).classes('w-full h-full')

    with ui.row().classes('w-full items-center justify-between bg-white border-t px-3 py-1'):
        status_label = ui.label('端口(已选/所有): 0/0,  号码: 1').classes('text-xs text-gray-600 font-mono')
        ui.label('发短信 ■   日志■').classes('text-xs text-gray-400')

    with ui.element('div').classes('w-full bg-black overflow-y-auto overflow-x-hidden flex-none').style('height:130px'):
        log_html = ui.html('').classes('font-mono p-1')


with ui.dialog() as settings_dialog, ui.card().classes('w-80'):
    ui.label('参数设置').classes('text-base font-bold mb-2')
    ui.input('服务器地址').bind_value(app_state, 'server_url').props('dense outlined').classes('w-full')
    ui.input('用户名').bind_value(app_state, 'username').props('dense outlined').classes('w-full')
    ui.input('密码', password=True, password_toggle_button=True).bind_value(app_state, 'password').props('dense outlined').classes('w-full')
    with ui.row().classes('w-full justify-end gap-2 mt-2'):
        ui.button('取消', on_click=settings_dialog.close).props('flat')
        ui.button('确定', on_click=settings_dialog.close).classes('btn-action')


ui.timer(10.0, _auto_poll)
ui.timer(0.5, refresh_all, once=True)

if __name__ in {'__main__', '__mp_main__'}:
    ui.run(title='SMS Gateway Monitor', port=9000, dark=False)