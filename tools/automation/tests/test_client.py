import base64
import json
from pathlib import Path
import socket
import tempfile
import threading
import unittest
import sys
sys.path.insert(0,str(Path(__file__).resolve().parents[1]))
from bedtime import Client, AutomationError, Capture


class ClientTests(unittest.TestCase):
    def serve(self, replies):
        client,server=socket.socketpair()
        requests=[]
        def run():
            with server,server.makefile('rb') as reader:
                for reply in replies:
                    request=json.loads(reader.readline())
                    requests.append(request)
                    payload=json.dumps(dict(id=request['id'],**reply)).encode()+b'\n'
                    # Split JSON and its delimiter across reads.
                    server.sendall(payload[:3]);server.sendall(payload[3:])
        worker=threading.Thread(target=run)
        worker.start()
        self.addCleanup(worker.join,2)
        app=Client(client)
        self.addCleanup(app.close)
        return app,requests

    def test_call_waits_for_completion_and_releases_receipt(self):
        app,requests=self.serve([{'result':{'operation':9}},
            {'result':{'state':'pending'}},
            {'result':{'state':'complete','value':{'worked':True}}},
            {'result':{'released':True}}])
        self.assertEqual(app.call('fixture.operation',value=1),{'worked':True})
        self.assertEqual([r['method'] for r in requests],['submit','poll','poll','release'])

    def test_backend_failure_is_preserved_and_receipt_is_released(self):
        app,requests=self.serve([{'result':{'operation':9}},
            {'result':{'state':'complete','error':{'kind':'RevisionConflict','message':'stale'}}},
            {'result':{'released':True}}])
        with self.assertRaises(AutomationError) as error:
            app.call('fixture.operation')
        self.assertEqual(error.exception.kind,'RevisionConflict')
        self.assertEqual(requests[-1]['method'],'release')

    def test_receipt_result_is_cached_until_release(self):
        app,requests=self.serve([{'result':{'operation':9}},
            {'result':{'state':'complete','value':{'worked':True}}},
            {'result':{'released':True}}])
        operation=app.submit('fixture.operation')
        self.assertEqual(operation.wait(),operation.wait())
        operation.release();operation.release()
        self.assertEqual(len(requests),3)
        with self.assertRaises(RuntimeError):operation.wait()

    def test_png_is_decoded_and_saved_without_dependencies(self):
        with tempfile.TemporaryDirectory() as directory:
            data=b'\x89PNG\r\n\x1a\nfixture'
            capture=Capture({'frame':1},[{'png':base64.b64encode(data).decode()}])
            path=capture.save(Path(directory)/'nested/image.png')
            self.assertEqual(path.read_bytes(),data)

    def test_unknown_response_id_closes_the_connection(self):
        client,server=socket.socketpair()
        app=Client(client)
        self.addCleanup(server.close)
        server.sendall(b'{"id":999,"result":{}}\n')
        with self.assertRaises(ConnectionError):app._request('inspect',{})
        self.assertEqual(client.fileno(),-1)


if __name__=='__main__':unittest.main()
