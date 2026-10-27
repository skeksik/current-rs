namespace Current.Shell.Pages;

/// <summary>A page whose content the title bar's Refresh button (and the page's own F5 and Ctrl+R) reads again from its server.</summary>
internal interface IRefreshable
{
    void Refresh();
}
