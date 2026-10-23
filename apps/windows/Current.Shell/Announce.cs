using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation.Peers;

namespace Current.Shell;

/// <summary>
/// Makes Narrator read a live region that just appeared. A text with <c>AutomationProperties.LiveSetting</c> is only read when its
/// peer raises <c>LiveRegionChanged</c>; a fixed text such as "Signing in…" never changes, so it is raised when the text is shown.
/// </summary>
internal static class Announce
{
    public static void Shown(FrameworkElement element)
    {
        // After layout, so the element is visible to UI Automation when the event arrives.
        element.DispatcherQueue.TryEnqueue(() =>
        {
            var peer = FrameworkElementAutomationPeer.FromElement(element) ?? FrameworkElementAutomationPeer.CreatePeerForElement(element);
            peer?.RaiseAutomationEvent(AutomationEvents.LiveRegionChanged);
        });
    }
}
