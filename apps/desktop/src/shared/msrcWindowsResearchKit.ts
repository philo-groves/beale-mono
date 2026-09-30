import type { ScopeAssetInput, ScopeAssetKind } from './types';

export const MSRC_WINDOWS_PROGRAM_URL = 'https://www.microsoft.com/en-us/msrc/bounty-windows-insider-preview';
const WINDOWS_APP_OVERVIEW_URL = 'https://learn.microsoft.com/en-us/windows/whats-new/windows-11-overview';
const WINDOWS_APP_INVENTORY_URL = 'https://learn.microsoft.com/en-us/windows/application-management/msix-app-packaging-tool';
const WINDOWS_SERVICE_INVENTORY_URL = 'https://learn.microsoft.com/en-us/windows/iot/iot-enterprise/optimize/services';

function resource(
  kind: ScopeAssetKind,
  value: string,
  displayName: string,
  catalogGroup: 'Windows apps' | 'Windows services' | 'Attack-scenario sandboxes' | 'Shipped open source',
  sourceUrl: string,
  note: string
): ScopeAssetInput {
  return {
    direction: 'in_scope',
    kind,
    value,
    sensitivity: 'public',
    attributes: {
      source: 'msrc-windows',
      researchKitId: 'msrc',
      researchKitSourceUrl: sourceUrl,
      displayName,
      catalogGroup,
      catalogNote: note,
      ...(kind === 'repo' ? { repositoryUrl: value } : {})
    }
  };
}

export const MSRC_WINDOWS_RESOURCES: readonly ScopeAssetInput[] = [
  resource('binary', 'explorer.exe', 'File Explorer', 'Windows apps', WINDOWS_APP_OVERVIEW_URL, 'Verify the binary and serviced feature on the selected Canary guest.'),
  resource('binary', 'msedge.exe', 'Microsoft Edge', 'Windows apps', WINDOWS_APP_OVERVIEW_URL, 'Edge is included with Windows 11; verify its installed version and the applicable bounty program.'),
  resource('binary', 'conhost.exe', 'Windows Console Host', 'Windows apps', 'https://github.com/microsoft/terminal', 'Inbox console host; compare the guest binary with the corresponding source revision.'),
  resource('binary', 'cmd.exe', 'Command Prompt', 'Windows apps', WINDOWS_APP_OVERVIEW_URL, 'Verify the binary and serviced feature on the selected Canary guest.'),
  resource('binary', 'powershell.exe', 'Windows PowerShell', 'Windows apps', WINDOWS_APP_OVERVIEW_URL, 'Windows PowerShell is distinct from the separately installed open-source PowerShell 7.'),
  resource('binary', 'SystemSettings.exe', 'Windows Settings', 'Windows apps', WINDOWS_APP_OVERVIEW_URL, 'Verify the binary and serviced feature on the selected Canary guest.'),
  resource('binary', 'notepad.exe', 'Notepad', 'Windows apps', WINDOWS_APP_INVENTORY_URL, 'Store-provisioned app availability and version can vary by image and update state.'),
  resource('binary', 'mspaint.exe', 'Paint', 'Windows apps', WINDOWS_APP_INVENTORY_URL, 'Store-provisioned app availability and version can vary by image and update state.'),
  resource('binary', 'SnippingTool.exe', 'Snipping Tool', 'Windows apps', WINDOWS_APP_INVENTORY_URL, 'Store-provisioned app availability and version can vary by image and update state.'),
  resource('binary', 'Microsoft.WindowsCalculator', 'Calculator', 'Windows apps', 'https://github.com/microsoft/calculator', 'Check the provisioned package and installed version on the guest.'),
  resource('binary', 'Microsoft.WindowsTerminal', 'Windows Terminal', 'Windows apps', 'https://github.com/microsoft/terminal', 'Check the provisioned package and installed version on the guest.'),
  resource('binary', 'Microsoft.DesktopAppInstaller', 'App Installer / WinGet', 'Windows apps', 'https://github.com/microsoft/winget-cli', 'Check that App Installer and winget are present on the guest.'),
  resource('binary', 'Microsoft.WindowsStore', 'Microsoft Store', 'Windows apps', WINDOWS_APP_INVENTORY_URL, 'Check the provisioned package and installed version on the guest.'),
  resource('binary', 'Microsoft.Windows.Photos', 'Photos', 'Windows apps', WINDOWS_APP_INVENTORY_URL, 'Store-provisioned app availability and version can vary by image and update state.'),
  resource('binary', 'Microsoft.SecHealthUI', 'Windows Security', 'Windows apps', WINDOWS_APP_INVENTORY_URL, 'Check the provisioned package and security configuration on the guest.'),
  resource('service', 'WinDefend', 'Microsoft Defender Antivirus Service', 'Windows services', WINDOWS_SERVICE_INVENTORY_URL, 'Check service presence and security configuration on the guest.'),
  resource('service', 'mpssvc', 'Microsoft Defender Firewall', 'Windows services', WINDOWS_SERVICE_INVENTORY_URL, 'Check service presence and security configuration on the guest.'),
  resource('service', 'wuauserv', 'Windows Update', 'Windows services', WINDOWS_SERVICE_INVENTORY_URL, 'Check service presence and startup state on the guest.'),
  resource('service', 'Schedule', 'Task Scheduler', 'Windows services', WINDOWS_SERVICE_INVENTORY_URL, 'Check service presence and startup state on the guest.'),
  resource('service', 'EventLog', 'Windows Event Log', 'Windows services', WINDOWS_SERVICE_INVENTORY_URL, 'Check service presence and startup state on the guest.'),
  resource('service', 'RpcSs', 'Remote Procedure Call', 'Windows services', WINDOWS_SERVICE_INVENTORY_URL, 'Check service presence and startup state on the guest.'),
  resource('service', 'Dnscache', 'DNS Client', 'Windows services', WINDOWS_SERVICE_INVENTORY_URL, 'Check service presence and startup state on the guest.'),
  resource('service', 'Dhcp', 'DHCP Client', 'Windows services', WINDOWS_SERVICE_INVENTORY_URL, 'Check service presence and startup state on the guest.'),
  resource('service', 'Spooler', 'Print Spooler', 'Windows services', WINDOWS_SERVICE_INVENTORY_URL, 'May be disabled or removed by the image or operator; verify before selecting.'),
  resource('service', 'BITS', 'Background Intelligent Transfer Service', 'Windows services', WINDOWS_SERVICE_INVENTORY_URL, 'Check service presence and startup state on the guest.'),
  resource('service', 'CryptSvc', 'Cryptographic Services', 'Windows services', WINDOWS_SERVICE_INVENTORY_URL, 'Check service presence and startup state on the guest.'),
  resource('service', 'AppXSvc', 'AppX Deployment Service', 'Windows services', WINDOWS_SERVICE_INVENTORY_URL, 'Check service presence and startup state on the guest.'),
  resource('service', 'WSearch', 'Windows Search', 'Windows services', WINDOWS_SERVICE_INVENTORY_URL, 'May be disabled by image policy; verify before selecting.'),
  resource('service', 'WinHttpAutoProxySvc', 'WinHTTP Web Proxy Auto-Discovery Service', 'Windows services', WINDOWS_SERVICE_INVENTORY_URL, 'Check service presence and whether it launches the sandboxed WPAD process.'),
  resource('binary', 'msedge.exe (renderer)', 'Microsoft Edge Chromium renderer sandbox', 'Attack-scenario sandboxes', MSRC_WINDOWS_PROGRAM_URL, 'Listed as an eligible sandbox for local attack-scenario awards; verify the renderer context.'),
  resource('binary', 'MsMpEngCP.exe', 'Windows Defender sandbox', 'Attack-scenario sandboxes', MSRC_WINDOWS_PROGRAM_URL, 'Listed as an eligible sandbox; verify that the sandboxed process is present and enabled.'),
  resource('binary', 'WinHTTP WPAD sandboxed process', 'WinHTTP WPAD sandbox', 'Attack-scenario sandboxes', MSRC_WINDOWS_PROGRAM_URL, 'The program names the sandboxed WPAD process; the WinHttpAutoProxySvc service alone does not prove that context.'),
  resource('binary', 'UtcDecoderHost.exe', 'UTC decoder sandbox', 'Attack-scenario sandboxes', MSRC_WINDOWS_PROGRAM_URL, 'Listed as an eligible sandbox; verify the sandboxed process and entry context.'),
  resource('repo', 'https://github.com/microsoft/terminal', 'Windows Terminal and Console Host source', 'Shipped open source', 'https://github.com/microsoft/terminal', 'Contains Windows Terminal and the source used to build inbox conhost.exe; match the guest revision.'),
  resource('repo', 'https://chromium.googlesource.com/chromium/src', 'Chromium source used by Microsoft Edge', 'Shipped open source', 'https://learn.microsoft.com/en-us/microsoft-edge/web-platform/site-impacting-changes', 'Large source checkout. Microsoft Edge includes Microsoft changes; match the affected code and installed Edge revision.'),
  resource('repo', 'https://github.com/microsoft/calculator', 'Windows Calculator source', 'Shipped open source', 'https://github.com/microsoft/calculator', 'Calculator ships preinstalled; some shipped functionality is proprietary and absent from this repository.'),
  resource('repo', 'https://github.com/microsoft/winget-cli', 'Windows Package Manager source', 'Shipped open source', 'https://github.com/microsoft/winget-cli', 'WinGet is delivered through App Installer; verify that package and its version on the guest.')
];
