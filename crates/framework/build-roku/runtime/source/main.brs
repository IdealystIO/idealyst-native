' Entry point. The Roku runtime calls Main() on app launch.
' We stand up the SceneGraph screen, show our top-level scene
' (IdealystScene, which loads pkg:/data/ui.json on init), and
' pump the event loop until the user exits.

sub Main()
    screen = createObject("roSGScreen")
    port = createObject("roMessagePort")
    screen.setMessagePort(port)
    screen.createScene("IdealystScene")
    screen.show()

    while true
        msg = wait(0, port)
        msgType = type(msg)
        if msgType = "roSGScreenEvent" then
            if msg.isScreenClosed() then return
        end if
    end while
end sub
