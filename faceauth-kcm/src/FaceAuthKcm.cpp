#include "FaceAuthKcm.h"

#include <KPluginFactory>

K_PLUGIN_CLASS_WITH_JSON(FaceAuthKcm, "kcm_faceauth.json")

FaceAuthKcm::FaceAuthKcm(QObject *parent, const KPluginMetaData &data)
    : KQuickConfigModule(parent, data)
    , m_backend(new FaceAuthBackend(this))
{
    // No Apply / Cancel / Defaults toolbar buttons — every action in this KCM
    // is applied immediately to the daemon, matching the CLI behaviour.
    setButtons(KQuickConfigModule::NoAdditionalButton);
}

#include "FaceAuthKcm.moc"
