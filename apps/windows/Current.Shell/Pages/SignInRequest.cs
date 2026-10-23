using System.Diagnostics;
using Current.Bindings;
using Current.Shell.ViewModels;
using Microsoft.UI.Xaml.Controls;

namespace Current.Shell.Pages;

/// <summary>
/// What <see cref="SignInPage"/> is navigated with. The page owns <see cref="Flow"/> from then on and disposes it when it is left.
/// </summary>
/// <param name="HttpsFailedTls">Setup found https with a certificate this PC does not trust and fell back to http (ADR 0012).</param>
/// <param name="UserName">The user to fill in, for signing in again; null for a new server.</param>
internal sealed record SignInRequest(SignInFlow Flow, string ServerName, bool HttpsFailedTls, string? UserName)
{
    /// <summary>Opens the sign-in page for a saved server that is signed out. Shows a notice if the core refuses.</summary>
    public static void SignInAgain(Frame frame, CoreHost core, ServerRow row)
    {
        SignInFlow flow;
        try
        {
            flow = core.SignInAgain(row.Server);
        }
        catch (ServerHasNoAddressException)
        {
            App.ShowNotice(InfoBarSeverity.Error, $"Can't sign in to {row.Name}.", "Current has no address saved for this server.");
            return;
        }
        catch (Exception error) when (error is UnknownServerException or CoreRuntimeException)
        {
            Trace.TraceError($"Current: sign in again failed: {error.GetType().Name}");
            App.ShowNotice(InfoBarSeverity.Error, $"Can't sign in to {row.Name}.", ProblemText.Unexpected);
            return;
        }
        if (!frame.Navigate(typeof(SignInPage), new SignInRequest(flow, row.Name, false, row.UserName)))
        {
            flow.Dispose();
        }
    }
}
