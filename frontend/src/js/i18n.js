import { writable, derived } from 'svelte/store';

// ── Translation dictionaries ──────────────────────────────────────────────────
const translations = {
  en: {
    // App
    app_tagline:              'Secure messaging platform',

    // Login
    login_welcome:            'Welcome back',
    login_subtitle:           'Sign in to continue to SMS Gateway',
    login_username:           'Username',
    login_username_ph:        'Enter your username',
    login_password:           'Password',
    login_password_ph:        'Enter your password',
    login_remember:           'Remember me',
    login_signing_in:         'Signing in...',
    login_sign_in:            'Sign in',
    login_err_required:       'Username and password are required',
    login_err_invalid:        'Invalid credentials',
    login_err_unexpected:     'Unexpected error: HTTP {status}',
    login_err_connect:        'Unable to connect to the server',
    login_err_auth:           'Authentication process failed',

    // Sidebar
    sim_dashboard:            'SIM Dashboard',
    sim_dashboard_sub:        'View all SIMs',
    call_log:                 'Call Log',
    call_log_sub_history:     'History & dial',
    call_in_progress:         'Call in progress',
    sim_cards:                'SIM Cards',
    sim_cards_sub:            'Manage Device Info',
    logout:                   'Logout',
    logout_sub:               'Safe Logout',
    lang_label:               'EN',
    lang_tooltip:             'Switch language',

    // ConversationList
    messages:                 'Messages',
    compose:                  'Compose',
    inbox:                    'Inbox',
    sent:                     'Sent',
    search_inbox:             'Search inbox...',
    search_sent:              'Search sent...',
    no_results:               'No results',
    no_messages:              'No messages',
    nothing_sent:             'Nothing sent yet',
    filter_by_sim:            'Filtered by SIM: {sim}',
    clear_sim_filter:         'Show all',
    no_messages_for_sim:      'No messages for this SIM',
    nothing_sent_for_sim:     'Nothing sent from this SIM',

    // MessageHeader
    to_label:                 'To:',

    // MessageInput
    enter_contact_first:      'Enter contact first',
    type_message:             'Type your message...',
    select_sim_first:         'Please select a SIM card first',
    send:                     'Send',
    confirm_message:          'Confirm Message',
    confirm_subtitle:         'Review message details before sending',
    sending_from:             'Sending From',
    not_selected:             'Not Selected',
    active_label:             'Active',
    message_content:          'Message Content',
    sms_segments:             '{n} SMS',
    message_too_long:         'Message exceeds 70 characters — will be split into {n} SMS segments. Carrier charges may apply per segment.',
    sms_disclaimer:           'Standard SMS rates apply \u2022 Message sends immediately',
    cancel:                   'Cancel',
    sending:                  'Sending...',

    // SendDialog
    send_new_message:         'New Message',
    select_device:            'Select Device:',
    select_sim_ph:            'Select SIM card',
    recipient:                'Recipient:',
    enter_phone:              'Enter phone number',
    message_label:            'Message:',
    enter_message:            'Enter message (max 500 chars)',
    err_select_sim:           'Please select a SIM card',
    err_invalid_phone:        'Please enter a valid phone number',
    err_empty_message:        'Please enter a message',
    err_send_failed:          'Send failed, please try again',

    // CallLog
    call_log_title:           'Call Log',
    sse_connected:            'connected',
    sse_disconnected:         'disconnected',
    make_a_call:              'Make a call',
    phone_number_ph:          'Phone number',
    no_calls:                 'No calls yet',
    unknown:                  'Unknown',

    // IncomingCallBanner
    incoming_call:            'Incoming call',
    calling:                  'Calling\u2026',
    call_in_progress_banner:  'Call in progress',
    unknown_number:           'Unknown number',

    // Dashboard
    close:                    'Close',
    conversations:            'Conversations',

    // MessageItem
    code_copied:              'Code copied',

    // SimDashboard
    sim_dashboard_title:      'SIM Dashboard',
    sim_count:                '{n} SIM',
    sim_count_plural:         '{n} SIMs',
    selected_count:           '{n} selected',
    btn_messages:             'Messages',
    btn_refresh:              'Refresh',
    btn_logout:               'Logout',
    col_com_port:             'COM Port',
    col_module:               'Module',
    col_signal:               'Signal',
    col_network_status:       'Network Status',
    col_phone_number:         'Phone Number',
    col_operator:             'Operator',
    col_sms_recv:             'SMS Recv',
    col_sms_sent:             'SMS Sent',
    col_country:              'Country',
    col_sms:                  'SMS',
    col_imsi:                 'IMSI',
    col_iccid:                'ICCID',
    col_imei:                 'IMEI',
    net_not_registered:       'Not Registered',
    net_home:                 'Home',
    net_searching:            'Searching',
    net_denied:               'Denied',
    net_unknown:              'Unknown',
    net_roaming:              'Roaming',
    net_code:                 'Code {n}',
    no_sim:                   'No SIM',
    unavailable:              'Unavailable',
    no_sim_cards:             'No SIM cards found',
    err_load_sim:             'Failed to load SIM data',

    // SimCard Info
    sim_card_information:     'SIM Card Information',
    basic_information:        'Basic Information',
    technical_details:        'Technical Details',
    enhanced_information:     'Enhanced Information',
    signal_strength:          'Signal Strength',
    alias_label:              'Alias',
    not_set:                  'Not set',
    try_again:                'Try Again',
    refresh_sim_info:         'Refresh SIM info',
    no_data_available:        'No Data Available',
    no_data_description:      'Unable to load information for this SIM card.',
    no_sim_available:         'No SIM cards are currently available.',
    sms_center_label:         'SMS Center:',
    sim_status_label:         'SIM Status:',
    memory_label:             'Memory:',
    port_label:               'Port:',
    signal_detail_label:      'Signal Detail:',
    operator_id_label:        'Operator ID:',
    net_reg_home:             'Registered (Home)',
    net_reg_denied:           'Registration denied',
    net_reg_roaming:          'Registered (Roaming)',
    sig_no_signal:            'No Signal',
    sig_very_poor:            'Very Poor',
    sig_poor:                 'Poor',
    sig_moderate:             'Moderate',
    sig_good:                 'Good',
    sig_excellent:            'Excellent',
  },

  zh: {
    // App
    app_tagline:              '安全短信平台',

    // Login
    login_welcome:            '欢迎回来',
    login_subtitle:           '登录 SMS Gateway',
    login_username:           '用户名',
    login_username_ph:        '请输入用户名',
    login_password:           '密码',
    login_password_ph:        '请输入密码',
    login_remember:           '记住我',
    login_signing_in:         '登录中...',
    login_sign_in:            '登录',
    login_err_required:       '用户名和密码不能为空',
    login_err_invalid:        '用户名或密码错误',
    login_err_unexpected:     '未知错误：HTTP {status}',
    login_err_connect:        '无法连接到服务器',
    login_err_auth:           '身份验证失败',

    // Sidebar
    sim_dashboard:            'SIM 仪表盘',
    sim_dashboard_sub:        '查看所有 SIM',
    call_log:                 '通话记录',
    call_log_sub_history:     '历史与拨号',
    call_in_progress:         '通话中',
    sim_cards:                'SIM 卡',
    sim_cards_sub:            '管理设备信息',
    logout:                   '退出登录',
    logout_sub:               '安全退出',
    lang_label:               '中',
    lang_tooltip:             '切换语言',

    // ConversationList
    messages:                 '消息',
    compose:                  '写信',
    inbox:                    '收件箱',
    sent:                     '已发送',
    search_inbox:             '搜索收件箱...',
    search_sent:              '搜索已发送...',
    no_results:               '无结果',
    no_messages:              '暂无消息',
    nothing_sent:             '暂无已发送消息',
    filter_by_sim:            '已筛选 SIM：{sim}',
    clear_sim_filter:         '显示全部',
    no_messages_for_sim:      '该 SIM 暂无收到消息',
    nothing_sent_for_sim:     '该 SIM 暂无已发送消息',

    // MessageHeader
    to_label:                 '收件人：',

    // MessageInput
    enter_contact_first:      '请先输入联系人',
    type_message:             '输入消息...',
    select_sim_first:         '请先选择 SIM 卡',
    send:                     '发送',
    confirm_message:          '确认发送',
    confirm_subtitle:         '发送前请确认消息详情',
    sending_from:             '发送方',
    not_selected:             '未选择',
    active_label:             '活跃',
    message_content:          '消息内容',
    sms_segments:             '{n} 条短信',
    message_too_long:         '消息超过 70 个字符，将拆分为 {n} 条短信。运营商可能按条计费。',
    sms_disclaimer:           '标准短信费率适用 \u2022 消息立即发送',
    cancel:                   '取消',
    sending:                  '发送中...',

    // SendDialog
    send_new_message:         '发送新消息',
    select_device:            '选择设备：',
    select_sim_ph:            '请选择SIM卡',
    recipient:                '接收号码：',
    enter_phone:              '请输入电话号码',
    message_label:            '消息内容：',
    enter_message:            '请输入消息内容（最多500字）',
    err_select_sim:           '请选择SIM卡',
    err_invalid_phone:        '请输入有效的电话号码',
    err_empty_message:        '请输入消息内容',
    err_send_failed:          '发送失败，请稍后重试',

    // CallLog
    call_log_title:           '通话记录',
    sse_connected:            '已连接',
    sse_disconnected:         '未连接',
    make_a_call:              '拨打电话',
    phone_number_ph:          '电话号码',
    no_calls:                 '暂无通话记录',
    unknown:                  '未知',

    // IncomingCallBanner
    incoming_call:            '来电',
    calling:                  '拨号中…',
    call_in_progress_banner:  '通话中',
    unknown_number:           '未知号码',

    // Dashboard
    close:                    '关闭',
    conversations:            '对话',

    // MessageItem
    code_copied:              '验证码已复制',

    // SimDashboard
    sim_dashboard_title:      'SIM 仪表盘',
    sim_count:                '{n} 张SIM',
    sim_count_plural:         '{n} 张SIM',
    selected_count:           '已选 {n}',
    btn_messages:             '消息',
    btn_refresh:              '刷新',
    btn_logout:               '退出',
    col_com_port:             'COM端口',
    col_module:               '模块',
    col_signal:               '信号',
    col_network_status:       '网络状态',
    col_phone_number:         '电话号码',
    col_operator:             '运营商',
    col_sms_recv:             '收到短信',
    col_sms_sent:             '发送短信',
    col_country:              '国家',
    col_sms:                  '短信',
    col_imsi:                 'IMSI',
    col_iccid:                'ICCID',
    col_imei:                 'IMEI',
    net_not_registered:       '未注册',
    net_home:                 '本地',
    net_searching:            '搜索中',
    net_denied:               '被拒绝',
    net_unknown:              '未知',
    net_roaming:              '漫游',
    net_code:                 '代码 {n}',
    no_sim:                   '无SIM卡',
    unavailable:              '不可用',
    no_sim_cards:             '未找到SIM卡',
    err_load_sim:             '加载SIM数据失败',

    // SimCard Info
    sim_card_information:     'SIM卡信息',
    basic_information:        '基本信息',
    technical_details:        '技术详情',
    enhanced_information:     '增强信息',
    signal_strength:          '信号强度',
    alias_label:              '别名',
    not_set:                  '未设置',
    try_again:                '重试',
    refresh_sim_info:         '刷新SIM信息',
    no_data_available:        '无可用数据',
    no_data_description:      '无法加载此SIM卡的信息。',
    no_sim_available:         '当前没有可用的SIM卡。',
    sms_center_label:         '短信中心：',
    sim_status_label:         'SIM状态：',
    memory_label:             '内存：',
    port_label:               '端口：',
    signal_detail_label:      '信号详情：',
    operator_id_label:        '运营商ID：',
    net_reg_home:             '已注册（本地）',
    net_reg_denied:           '注册被拒绝',
    net_reg_roaming:          '已注册（漫游）',
    sig_no_signal:            '无信号',
    sig_very_poor:            '极差',
    sig_poor:                 '差',
    sig_moderate:             '一般',
    sig_good:                 '良好',
    sig_excellent:            '优秀',
  },
};

// ── Store ─────────────────────────────────────────────────────────────────────
const stored = typeof localStorage !== 'undefined'
  ? (localStorage.getItem('lang') || 'en')
  : 'en';

export const lang = writable(stored);

lang.subscribe(v => {
  if (typeof localStorage !== 'undefined') localStorage.setItem('lang', v);
});

// ── Reactive translate function ───────────────────────────────────────────────
// Usage in components: import { t } from '...'; then $t('key') or $t('key', { n: 3 })
export const t = derived(lang, ($lang) => (key, params = {}) => {
  const dict = translations[$lang] || translations.en;
  let str = dict[key] ?? translations.en[key] ?? key;
  for (const [k, v] of Object.entries(params)) {
    str = str.replace(`{${k}}`, String(v));
  }
  return str;
});

export function toggleLang() {
  lang.update(l => (l === 'en' ? 'zh' : 'en'));
}
