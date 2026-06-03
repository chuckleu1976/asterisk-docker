with open("/home/asterisk-build/asterisk/res/res_pjsip_messaging.c", "r") as f:
    content = f.read()

SELECTOR_FIX = (
    "\t/* Use the established VoLTE TCP connection instead of letting PJSIP\n"
    "\t * create a new socket (which fails with EADDRINUSE since the client\n"
    "\t * port is already bound by the registered TCP connection). */\n"
    "\tao2_lock(transport_state);\n"
    "\tif (transport_state->volte.transport) {\n"
    "\t\tpjsip_tpselector selector;\n"
    "\t\tpj_bzero(&selector, sizeof(selector));\n"
    "\t\tselector.type = PJSIP_TPSELECTOR_TRANSPORT;\n"
    "\t\tselector.u.transport = transport_state->volte.transport;\n"
    "\t\tpjsip_tx_data_set_transport(tdata, &selector);\n"
    "\t}\n"
    "\tao2_unlock(transport_state);\n"
    "\n"
)

patched = 0

# Patch 1: send_rpack (returns PJ_SUCCESS / status, followed by parse_rpdata)
old1 = (
    "\tstatus = ast_sip_send_request(tdata, NULL, endpoint, NULL, NULL);\n"
    "\tif (status) {\n"
    "\t\tast_log(LOG_ERROR, \"PJSIP MESSAGE - Could not send request\\n\");\n"
    "\t\treturn status;\n"
    "\t}\n"
    "\n"
    "\treturn PJ_SUCCESS;\n"
    "}\n"
    "\n"
    "static void parse_rpdata"
)
new1 = SELECTOR_FIX + old1
# Only patch if not already patched
if "Use the established VoLTE" not in content[:content.find("static void parse_rpdata")]:
    if old1 in content:
        content = content.replace(old1, new1, 1)
        patched += 1
        print("Patch 1 (send_rpack): OK")
    else:
        print("Patch 1 (send_rpack): NOT FOUND")
else:
    print("Patch 1 (send_rpack): already patched")

# Patch 2: volte_send_rp_data (returns -1/0, followed by is_7bit_compatible)
# Note: "return 0;" has a trailing tab in the source
old2 = (
    "\tstatus = ast_sip_send_request(tdata, NULL, endpoint, NULL, NULL);\n"
    "\tif (status) {\n"
    "\t\tast_log(LOG_ERROR, \"PJSIP MESSAGE - Could not send request\\n\");\n"
    "\t\treturn -1;\n"
    "\t}\n"
    "\n"
    "\treturn 0;\t\n"
    "}\n"
    "\n"
    "static pj_bool_t is_7bit_compatible"
)
new2 = SELECTOR_FIX + old2.replace("\treturn 0;\t\n", "\treturn 0;\n")
if "Use the established VoLTE" not in content[content.find("static pj_bool_t is_7bit_compatible")-3000:content.find("static pj_bool_t is_7bit_compatible")]:
    if old2 in content:
        content = content.replace(old2, new2, 1)
        patched += 1
        print("Patch 2 (volte_send_rp_data): OK")
    else:
        print("Patch 2 (volte_send_rp_data): NOT FOUND")
else:
    print("Patch 2 (volte_send_rp_data): already patched")

if patched > 0:
    with open("/home/asterisk-build/asterisk/res/res_pjsip_messaging.c", "w") as f:
        f.write(content)
    print(f"Wrote {patched} patch(es) to file")
    # Verify
    count = content.count("Use the established VoLTE")
    print(f"Verification: comment appears {count} times (expected 2)")
else:
    print("Nothing to patch")
