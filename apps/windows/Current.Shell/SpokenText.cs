using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Automation.Peers;
using Microsoft.UI.Xaml.Controls;

namespace Current.Shell;

/// <summary>
/// Shows its children but gives UI Automation only its <c>AutomationProperties.Name</c>, as one text element without children, for
/// text that should be heard differently from how it is shown. A named TextBlock is read twice in Narrator's scan mode, by its name
/// and then by its text (owner's run, 2026-10-24: the Quick Connect code's digits, then the code as one number). TextBlock is sealed
/// ("public sealed class TextBlock", TextBlock page), so it cannot get its own peer; a class that derives from one that is not (Grid)
/// overrides OnCreateAutomationPeer, and its peer's children are what GetChildrenCore returns ("Custom automation peers"); all on
/// learn.microsoft.com, read 2026-10-24.
/// </summary>
public sealed partial class SpokenText : Grid
{
    protected override AutomationPeer OnCreateAutomationPeer() => new Peer(this);

    private sealed partial class Peer(SpokenText owner) : FrameworkElementAutomationPeer(owner)
    {
        protected override AutomationControlType GetAutomationControlTypeCore() => AutomationControlType.Text;

        protected override string GetClassNameCore() => nameof(SpokenText);

        protected override string GetNameCore() => AutomationProperties.GetName(Owner);

        // No children: the shown text is not read a second time.
        protected override IList<AutomationPeer> GetChildrenCore() => [];
    }
}
