<!-- frontend/src/lib/components/simcard/SimCardTechnicalInfo.svelte -->
<script>
    import SimCardEnhancedInfo from "./SimCardEnhancedInfo.svelte";
    import { t } from "../../js/i18n.js";
    
    let { simInfo = null } = $props();
</script>

<div class="space-y-4">
    <h4 class="text-md font-semibold text-gray-700 dark:text-gray-300 border-b border-gray-200 dark:border-gray-600 pb-2">
        {$t('technical_details')}
    </h4>
    
    <SimCardEnhancedInfo {simInfo} />
</div>