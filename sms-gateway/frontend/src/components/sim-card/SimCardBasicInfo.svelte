<!-- frontend/src/lib/components/simcard/SimCardBasicInfo.svelte -->
<script>
    import Icon from "@iconify/svelte";
    import EditableField from "../common/EditableField.svelte";
    import { t } from "../../js/i18n.js";
    
    let {
        simCard = {},
        simInfo = null,
        onUpdatePhone = async (phone) => true,
        onUpdateAlias = async (alias) => true
    } = $props();
    
    const statusKeyMap = {
        "0": "net_not_registered",
        "1": "net_reg_home",
        "2": "net_searching",
        "3": "net_reg_denied",
        "5": "net_reg_roaming"
    };

    function getStatusKey(status) {
        return statusKeyMap[String(status)] ?? null;
    }
</script>

<div class="space-y-4">
    <h4 class="text-md font-semibold text-gray-700 dark:text-gray-300 border-b border-gray-200 dark:border-gray-600 pb-2">
        {$t('basic_information')}
    </h4>
    
    <EditableField
        value={simCard.phone_number}
        icon="mage:phone"
        label={$t('col_phone_number')}
        placeholder={$t('not_set')}
        onSave={onUpdatePhone}
    />
    
    <EditableField
        value={simCard.alias}
        icon="mage:tag"
        label={$t('alias_label')}
        placeholder={$t('not_set')}
        onSave={onUpdateAlias}
    />
    
    <div class="flex items-center">
        <Icon icon="mage:globe" class="w-5 h-5 mr-3 text-gray-500 dark:text-gray-400" />
        <div>
            <div class="text-xs text-gray-500 dark:text-gray-400">{$t('col_network_status')}</div>
            <div class="text-sm font-medium dark:text-gray-300">
                {#if simInfo?.network_registration?.status != null}
                    {$t(getStatusKey(simInfo.network_registration.status) ?? 'unknown')}
                {:else}
                    {$t('unknown')}
                {/if}
            </div>
        </div>
    </div>
    
    <div class="flex items-center">
        <Icon icon="mage:building-b" class="w-5 h-5 mr-3 text-gray-500 dark:text-gray-400" />
        <div>
            <div class="text-xs text-gray-500 dark:text-gray-400">{$t('col_operator')}</div>
            <div class="text-sm font-medium dark:text-gray-300">
                {simInfo?.operator_info?.operator_name || $t('unknown')}
            </div>
        </div>
    </div>
</div>