using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace Current.Shell.Gateway;

/// <summary>
/// The password prompt of a password gateway (HTTP Basic, ADR 0017, "Mechanism 1"): a ContentDialog, as the plain-http and remove
/// confirmations are (Dialogs page, learn.microsoft.com, ms.date 2026-07-15, read 2026-10-23). It names the origin (scheme, host and
/// port). The realm is the site's own text, so it shows only as "The site says: ..." in the body, never as the title. A refused
/// password shows the prompt again with the error inside it. The password goes from the PasswordBox to the caller and nowhere else.
/// </summary>
internal static class GatewayPasswordDialog
{
    /// <summary>The longest realm shown: the site's text, cut so it cannot push the fields out of the dialog.</summary>
    private const int RealmMaxLength = 200;

    internal sealed record Answer(string User, string Password)
    {
        private bool PrintMembers(System.Text.StringBuilder builder)
        {
            builder.Append("[redacted]");
            return true;
        }
    }

    /// <summary>
    /// Shows the prompt; null when the user cancels. <paramref name="plainHttp"/>: plain http outside the local network, so the
    /// prompt says the password would travel unencrypted and its button says so; choosing it is the user's confirmation.
    /// </summary>
    public static async Task<Answer?> AskAsync(XamlRoot root, string origin, string? realm, bool wrong, bool plainHttp, string? user)
    {
        var content = new StackPanel { Spacing = 12 };
        if (wrong)
        {
            content.Children.Add(new InfoBar
            {
                Severity = InfoBarSeverity.Error,
                IsClosable = false,
                IsOpen = true,
                Message = "The user name or password is incorrect.",
            });
        }
        content.Children.Add(new TextBlock
        {
            TextWrapping = TextWrapping.Wrap,
            Text = $"{origin} asks for a user name and password.",
        });
        if (!string.IsNullOrWhiteSpace(realm))
        {
            var shown = realm.Length > RealmMaxLength ? realm[..RealmMaxLength] + "…" : realm;
            content.Children.Add(new TextBlock { TextWrapping = TextWrapping.Wrap, Text = $"The site says: “{shown}”" });
        }
        if (plainHttp)
        {
            content.Children.Add(new InfoBar
            {
                Severity = InfoBarSeverity.Warning,
                IsClosable = false,
                IsOpen = true,
                Message = "This address uses http, so the password would cross the internet unencrypted.",
            });
        }
        var userBox = new TextBox
        {
            Header = "User name",
            Text = user ?? "",
            IsSpellCheckEnabled = false,
            IsTextPredictionEnabled = false,
        };
        var passwordBox = new PasswordBox { Header = "Password" };
        content.Children.Add(userBox);
        content.Children.Add(passwordBox);

        var dialog = new ContentDialog
        {
            XamlRoot = root,
            Title = "Sign in",
            Content = content,
            PrimaryButtonText = plainHttp ? "Sign in without encryption" : "Sign in",
            CloseButtonText = "Cancel",
            DefaultButton = ContentDialogButton.Primary,
        };
        // The user name first; the password when it is filled in already (a refused password).
        dialog.Opened += (_, _) =>
            (userBox.Text.Length > 0 ? (Control)passwordBox : userBox).Focus(FocusState.Programmatic);
        var result = await dialog.ShowAsync();
        var answer = result == ContentDialogResult.Primary ? new Answer(userBox.Text, passwordBox.Password) : null;
        passwordBox.Password = "";
        return answer;
    }
}
