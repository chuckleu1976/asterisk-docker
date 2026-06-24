<!-- frontend/src/lib/components/simcard/SimCardEnhancedInfo.svelte -->
<script>
    import Icon from "@iconify/svelte";
    import { t } from "../../js/i18n.js";
    
    let { simInfo = null } = $props();
    
    const hasEnhancedInfo = $derived(
        simInfo && (
            simInfo.sms_center || 
            simInfo.sim_status || 
            simInfo.memory_status || 
            simInfo.com_port || 
            simInfo.operator_info?.operator_id
        )
    );
</script>

{#if hasEnhancedInfo}
    <div class="border-t border-gray-200 dark:border-gray-700 pt-4 space-y-3">
        <h5 class="text-sm font-medium text-gray-600 dark:text-gray-400">{$t('enhanced_information')}</h5>
        
        {#if simInfo.sms_center}
            <div class="flex items-center text-sm">
                <Icon icon="mage:message-dots" class="w-4 h-4 mr-2 text-gray-500" />
                <span class="text-gray-500 dark:text-gray-400">{$t('sms_center_label')}</span>
                <span class="font-medium dark:text-gray-300 ml-2">{simInfo.sms_center}</span>
            </div>
        {/if}
        
        {#if simInfo.sim_status}
            <div class="flex items-center text-sm">
                <Icon icon="mage:shield-check" class="w-4 h-4 mr-2 text-gray-500" />
                <span class="text-gray-500 dark:text-gray-400">{$t('sim_status_label')}</span>
                <span class="font-medium dark:text-gray-300 ml-2">{simInfo.sim_status}</span>
            </div>
        {/if}
        
        {#if simInfo.memory_status}
            <div class="flex items-center text-sm">
                <Icon icon="mage:memory-card" class="w-4 h-4 mr-2 text-gray-500" />
                <span class="text-gray-500 dark:text-gray-400">{$t('memory_label')}</span>
                <span class="font-medium dark:text-gray-300 ml-2">{simInfo.memory_status}</span>
            </div>
        {/if}
        
        {#if simInfo.com_port}
            <div class="flex items-center text-sm">
                <Icon icon="mage:link" class="w-4 h-4 mr-2 text-gray-500" />
                <span class="text-gray-500 dark:text-gray-400">{$t('port_label')}</span>
                <span class="font-medium dark:text-gray-300 ml-2">{simInfo.com_port} @ {simInfo.baud_rate}</span>
            </div>
        {/if}
        
        {#if simInfo.operator_info?.operator_id}
            <div class="flex items-center text-sm">
                <Icon icon="mage:building-b" class="w-4 h-4 mr-2 text-gray-500" />
                <span class="text-gray-500 dark:text-gray-400">{$t('operator_id_label')}</span>
                <span class="font-medium dark:text-gray-300 ml-2">{simInfo.operator_info.operator_id}</span>
            </div>
        {/if}
    </div>
{/if}