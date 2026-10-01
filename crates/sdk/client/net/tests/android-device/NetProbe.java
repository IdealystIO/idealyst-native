// Entry point `run.sh` starts with `app_process`: loads the probe library
// and prints its report. HttpURLConnection is part of the framework
// classpath, so no APK, manifest or Android Context is needed.
public class NetProbe {
    static native String run();

    public static void main(String[] args) {
        System.load(args[0]);
        System.out.println(run());
        // A case whose timeout was ignored leaves a JVM-attached worker
        // thread blocked forever; it would keep app_process alive.
        System.exit(0);
    }
}
