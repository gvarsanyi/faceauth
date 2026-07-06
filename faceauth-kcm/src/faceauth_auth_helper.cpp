// Minimal KAuth helper for the org.faceauth polkit action.
// The PolkitQt1 backend requires a registered helper binary; the actual
// privileged work is done via the daemon socket, not here.
#include <KAuth/ActionReply>
#include <KAuth/HelperSupport>

class FaceAuthHelper : public QObject
{
    Q_OBJECT
public Q_SLOTS:
    KAuth::ActionReply manage(const QVariantMap &)
    {
        return KAuth::ActionReply::SuccessReply();
    }
};

KAUTH_HELPER_MAIN("org.faceauth", FaceAuthHelper)

#include "faceauth_auth_helper.moc"
