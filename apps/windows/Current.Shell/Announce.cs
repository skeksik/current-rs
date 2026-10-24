using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation.Peers;

namespace Current.Shell;

/// <summary>
/// Makes Narrator read what just appeared. A text with <c>AutomationProperties.LiveSetting</c> is only read when its
/// peer raises <c>LiveRegionChanged</c>; a fixed text such as "Signing in…" never changes, so it is raised when the text is shown.
/// </summary>
internal static class Announce
{
    public static void Shown(FrameworkElement element)
    {
        // After layout, so the element is visible to UI Automation when the event arrives.
        element.DispatcherQueue.TryEnqueue(() => PeerOf(element)?.RaiseAutomationEvent(AutomationEvents.LiveRegionChanged));
    }

    /// <summary>
    /// Has Narrator speak <paramref name="text"/> itself, for when the words to hear differ from the visible text: a live region
    /// is read by its text, not its name (owner's run, 2026-10-23: the Quick Connect code was read as a number). A newer
    /// notification with the same <paramref name="activityId"/> supersedes one not yet spoken ("ImportantMostRecent: … the most
    /// recent notification supersedes all of the other notifications", AutomationNotificationProcessing, learn.microsoft.com,
    /// read 2026-10-23).
    /// </summary>
    public static void Say(FrameworkElement element, string text, string activityId)
    {
        element.DispatcherQueue.TryEnqueue(() => PeerOf(element)?.RaiseNotificationEvent(
            AutomationNotificationKind.Other, AutomationNotificationProcessing.ImportantMostRecent, text, activityId));
    }

    private static AutomationPeer? PeerOf(FrameworkElement element) =>
        FrameworkElementAutomationPeer.FromElement(element) ?? FrameworkElementAutomationPeer.CreatePeerForElement(element);
}
